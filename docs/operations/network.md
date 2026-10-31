# Network

How to set up a host so uju's network path stays on the CPUs that own it.
None of this is needed for local development: the server listens on
`127.0.0.1`, and loopback traffic never touches a NIC.

> **Status.** Most of this describes the planned layout. Today every pinned
> thread is a `shard-N`, listening on `127.0.0.1:7000 + N` with the base
> hardcoded (`crates/uju/src/cluster/shard.rs`). Node kinds, edge threads,
> upstream ports and the client listener do not exist yet.

## Goal

Every uju node runs one thread per CPU it may use, pinned to that CPU
(`crates/uju/src/cluster/node.rs`): a shard on sim and hub nodes, an edge
thread on edge nodes (see [Topology](../architectures/topology.md)). A
packet's journey crosses three places that should all be the same CPU:

1. the **RX queue interrupt** and the softirq that runs the TCP/IP stack,
2. the **pinned thread** that receives it through io_uring,
3. the **TX completion** of what the thread sends back.

When they differ, every packet's data moves between CPUs' caches. The kernel
cannot align them on its own: the NIC picks a flow's RX queue by hashing its
4-tuple (RSS), and the queue's IRQ affinity picks the CPU. The steps below make
that choice land on the right thread.

Entry nodes are not covered here. They serve logins at a low rate behind an
ordinary load balancer, and gain nothing from steering.

## Requirements

- **Linux 6.1 or later.** Rings use `IORING_SETUP_SINGLE_ISSUER` (6.0) and
  `IORING_SETUP_DEFER_TASKRUN` (6.1), and compio also uses provided buffer
  rings (5.19) and multishot receives (6.0). uju builds compio with the
  io_uring driver only, so on an older kernel the runtime fails to start
  rather than falling back to epoll.
- **A multi-queue NIC**, with at least as many RX queues as pinned threads.
- **ntuple filters** (Intel ice/i40e, Mellanox mlx5, and most server NICs)
  for [ports](#ports). Without them, fall back to
  [RFS](#without-enough-queues-or-ntuple).
- Edge nodes may need more, depending on the
  [client listener](#client-listener).

## CPU set

The thread count is the size of the process's CPU affinity mask, read at
startup. Restrict it with `taskset -c`, systemd's `CPUAffinity=`, or a cgroup
cpuset.

The main thread shares the pinned threads' CPUs: it inherits the same mask,
and the scheduler places it on any of them. It only waits for shutdown, so
it costs them little. For the kernel's own work and IRQs that do not belong
to a pinned thread, leave at least one CPU out of the mask. The process
never runs there.

Thread `N` is the `N`th CPU of the mask, not CPU `N`. Each thread logs the
CPU it is pinned on at startup (`[shard-N] thread pinned on cpu C`). Use
that mapping for queues, IRQs and XPS below.

Prefer CPUs on the NIC's NUMA node:

```sh
cat /sys/class/net/eth0/device/numa_node
lscpu | grep NUMA
```

## Order

Each step can undo a later one, so apply them in this order, from a
boot-time unit after the link is up:

1. queue count (`ethtool -L`),
2. RSS spread (`ethtool -X`),
3. [ntuple rules](#ports),
4. [IRQ affinity](#irq-affinity),
5. [XPS](#tx).

`ethtool -L` and driver reloads reallocate IRQs and reset their affinity,
and some drivers, such as ice, delete ntuple rules that pointed at removed
queues.

## Ports

Sessions between nodes use ports of their own per thread, so the NIC can
steer them by port rather than by hash. Each base is a per-node setting, and
thread `N` uses `base + N`:

| Port | Thread | Use | Peers |
|---|---|---|---|
| shard | sim and hub shards | listen | edges, sim shards, hub shards, entry |
| upstream | sim and hub shards, edges | local port of outbound sessions | sim and hub shards |

Ports are registered in atlas's directory with their (node, thread). The
examples below use `9000` as the upstream base. Whether an edge has an
upstream port per thread or one per node depends on how its threads split
the work, which is not settled (see
[Inside an edge](../architectures/topology.md#inside-an-edge)).

Inbound sessions arrive on the listen port. Outbound sessions would
otherwise get an ephemeral local port that no filter can predict, so a
thread binds each outbound socket to its upstream port with `SO_REUSEADDR`
before connecting. All of them share that local port, which is fine because
their remote ends differ. This holds when:

- **Nothing listens on an upstream port.** Bind fails otherwise. Keep the
  upstream range out of ephemeral allocation with
  `net.ipv4.ip_local_reserved_ports`.
- **TIME_WAIT can be reused.** After a restart, an old connection with the
  same 4-tuple may still be in TIME_WAIT. Reusing it relies on TCP
  timestamps (`net.ipv4.tcp_timestamps`, on by default). Verify before
  relying on fast restarts.
- **A crash costs about a second.** After a crash, the peer may still hold
  the old connection as established. The new SYN draws a challenge ACK, the
  RST clears it, and the SYN is retransmitted after about a second.

Steer each of a thread's ports to its queue:

```sh
ethtool -K eth0 ntuple on

# sim or hub node: shard 0 → queue 0
ethtool -N eth0 flow-type tcp4 dst-port 7000 action 0   # inbound sessions
ethtool -N eth0 flow-type tcp4 dst-port 9000 action 0   # replies to outbound sessions

# edge node, with a port per thread: edge thread 0 → queue 0
ethtool -N eth0 flow-type tcp4 dst-port 9000 action 0   # replies from shards
```

Over IPv6, add the same rules with `flow-type tcp6`. Pair the rules with the
[IRQ affinity](#irq-affinity) of each queue.

## Client listener

> **Needs discussion.** Depends on how an edge's threads split the work
> (see [Inside an edge](../architectures/topology.md#inside-an-edge)).

Clients find an edge's address through entry. If any edge thread may serve
any client, there is one client port per edge node. Every edge thread binds
that address with `SO_REUSEPORT`, and sets `SO_INCOMING_CPU` to its own CPU.
The kernel hands a new connection to the listener whose CPU processed its
SYN, and falls back to the reuseport hash when no listener matches. A
connection then stays on that thread for its whole life.

RSS spreads clients evenly over the queues, and so over the threads. This
only pays off when RX queue IRQs land on edge threads' CPUs, one queue per
thread:

```sh
ethtool -L eth0 combined <threads>   # one queue per thread
ethtool -X eth0 equal <threads>      # spread RSS over exactly those queues
```

The ntuple rules for upstream ports take precedence over RSS, so both work
on the same NIC. A thread whose CPU takes no SYNs only receives the
connections that fall back to the hash, and those are processed off-CPU.

This needs:

- **Linux 6.2**, or a stable kernel with commit `b261eda84ec1`
  ("soreuseport: Fix socket selection for SO_INCOMING_CPU"), which is
  backported to 5.15. Before it, `SO_INCOMING_CPU` was ignored within a
  reuseport group.
- **No reuseport BPF program** on the group, which would replace this
  selection.
- **No RPS or RFS** on edge nodes. They move the SYN to another CPU, which
  then picks the listener.

## IRQ affinity

Stop irqbalance from moving the NIC's IRQs, then pin RX queue `N`'s IRQ to the
CPU of thread `N`:

```sh
systemctl disable --now irqbalance   # or ban the NIC's IRQs in its config

grep eth0 /proc/interrupts           # find each queue's IRQ number
echo <cpu> > /proc/irq/<irq>/smp_affinity_list
```

IRQ names depend on the driver (`eth0-TxRx-3`, `mlx5_comp3@pci:...`, ...), so
map queues to IRQs by name rather than by order.

## TX

Map each TX queue to its thread's CPU with XPS, so a thread sends on its own
queue and TX completions return to it:

```sh
# hex CPU mask; e.g. CPU 3 → 8
echo <mask> > /sys/class/net/eth0/queues/tx-<N>/xps_cpus
```

## Without enough queues or ntuple

RFS moves protocol processing to the CPU that last read the flow, after the
interrupt lands wherever RSS put it. It costs an IPI per batch, but recovers
most of the locality:

```sh
echo 32768 > /proc/sys/net/core/rps_sock_flow_entries
for q in /sys/class/net/eth0/queues/rx-*; do
    echo $((32768 / <queues>)) > $q/rps_flow_cnt
done
```

Accelerated RFS (aRFS) programs the same steering into the NIC. It needs the
NIC's ntuple support itself, so it helps when queues are short, not filters,
and on some drivers it competes with manual rules for the same table.

RFS does not combine with the [client listener](#client-listener).

## io_uring workers

Network I/O on the pinned threads' rings completes on the thread itself:
sockets are pollable, so nothing is punted to io_uring's kernel workers
(`iou-wrk-*`). Workers appearing under load point to blocking operations on
a ring and deserve a look:

```sh
ps -eLo pid,tid,psr,comm | grep -E 'shard-|edge-|iou-wrk'
```

Rings are built with `thread_pool_limit(0)`, so compio's own blocking pool
is off. Operations other than receives, sends, connects and accepts may
need it, so blocking work such as file I/O usually shows up as a compio
error before it shows up as `iou-wrk` threads.

`IORING_REGISTER_IOWQ_AFF` is not set yet, so workers are not guaranteed to
stay within the thread's CPU.

## Verifying

- `/proc/interrupts`: each queue's IRQ count grows on its thread's CPU only.
- `/proc/softirqs`: `NET_RX` grows on pinned CPUs, not on the ones left out.
- `ethtool -S eth0`: per-queue packet counters are roughly even.
- `ethtool -n eth0`: the ntuple rules are all still there after a queue
  change.
- `ps -eLo tid,psr,comm | grep -E 'shard-|edge-'`: each thread runs on the
  CPU it logged at startup (`psr`).
- `mpstat -P ALL -I SUM 1`: interrupt load per CPU.
