# Topology

How a uju cluster is laid out: who simulates the world, who owns state that
has no place in it, who talks to clients, and how state flows between them.

```
client ──login──► entry ──────────────────┐ presence
   │                │ token + edge address  │
   ▼                ▼                       ▼
client ─────────► edge ──┬──► sim node ── shard × N   owns cells
                         └──► hub node ── shard × N   owns slots
   sim shard ◄─► sim shard    sim shard ──► hub shard    hub shard ◄─► hub shard
  coordinator: placement maps, directory, every move
```

## Roles

- **Shard**: one thread pinned to one CPU, owning the state of a dynamic set
  of placement units (`crates/uju/src/runtime/shard.rs`). It is the
  authority for everything in them.
- **Sim node**: runs one shard per allowed CPU (`crates/uju/src/runtime/node.rs`).
  Its shards own **cells**, the squares of the world grid, and keep their
  entities in an ECS `World`.
- **Hub node**: a binary of its own, sharing only the shard runtime with
  sims. Its shards own **slots**, hash buckets of the state that has no
  position: parties, guilds, friends, chat channels, presence, mail. A hub
  shard holds no `World`: each slot keeps a map from key to object per
  kind, since hub objects are read and written one at a time by key, and no
  system iterates over them.
- **Edge**: one edge node, running one thread per allowed CPU. Terminates
  client connections, keeps replicas of the cells around its clients, does
  interest management and fan-out, routes requests to the owning shard, and
  answers requests the replicas alone can serve. The edge as a whole is the
  unit of layout. How its threads split the work is not settled yet (see
  [Inside an edge](#inside-an-edge)).
- **Entry**: the front door. Authenticates a client and tells it which edge
  to connect to.
- **Coordinator**: the control plane. Owns the placement maps and the
  directory of addresses, and decides every move of a cell, a slot or an
  edge cut.

A **replica** is a read-only copy of a cell, kept by a node that needs the
cell but does not own it: an edge for its clients, or a sim shard for the
cells bordering its own.

Thread counts are fixed for a process's lifetime. Nodes join and leave (see
[Membership](#membership)). Cells and slots move, threads do not.

## Placement

Three maps over two key spaces, each moved independently:

| Map | Changes | Driven by | Cost of a move |
|---|---|---|---|
| cell → sim shard | Often, automatic | Simulation cost | Internal state transfer |
| slot → hub shard | On scaling, automatic | Request rate, object count | Internal state transfer |
| cell → edge set | Gradually, automatic | Clients × AOI density | Client reconnects |

Sim and edge partition the same grid, but their loads differ: a dungeon full
of NPCs is heavy for the sim and light for the edge. Keeping the maps apart
lets each follow its own load, and lets sims rebalance without clients
noticing.

Coordinator decides every move. The owners carry it out between
themselves, and data never waits on coordinator.

## Cells

A cell's side is at least the largest **view radius** and the largest
**interaction range** in the game, plus a [handover
margin](#moving-between-edges). So anything an entity can see or touch lies
in its own cell or one of the eight around it.

That fixes the **halo at exactly one cell**, for edges and sim shards alike.
Nobody ever sees or acts past the halo, and "the halo of a set of cells" is
just the cells adjacent to it.

The price is large cells:

- **A cell is the smallest unit of balancing.** The worst crowd one cell can
  hold has to fit in one shard.
- **Interest is coarse.** A client receives the 3×3 cells around it, about
  nine times its view radius squared, against π times for a circle.

## Edges

### Why edges are spatial

- If every edge replicates the whole world, sim egress grows as shards ×
  edges, and every edge holds the whole world.
- If edges subscribe on demand but clients land on edges at random, each
  edge's clients are scattered over the world, and every edge ends up
  subscribing to nearly everything.

So clients are assigned to edges **by position**. That is what keeps each
edge's subscription small.

### Core and halo

Each edge owns a contiguous region of cells:

- **core**: cells whose clients this edge serves. Cores never overlap.
- **halo**: the one-cell ring around the core. Replicated, never serving
  clients.

```
cell     0  1  2  3  4  5  6  7  8
edge A   C  C  C  C  H
edge B            H  C  C  C  C  C        C core, H halo
```

Cells 3 and 4 are in both edges' replicas, which does two things:

1. Every client in a core has its whole view inside its edge's replicas.
2. A client crossing from A's core to B's sits inside both replicas, so it
   can move without a gap (see [Moving between edges](#moving-between-edges)).

A halo costs (k+2)²/k² for a core of k×k cells, so cores should span many
cells. This is why the core belongs to the whole edge rather than to each of
its threads: 16 threads each with their own 4×4 core would replicate 576
cells instead of the 324 of one 16×16 core, and turn every boundary between
them into a reconnect.

### Layout

Every cell lies in some edge's core, so an empty world still needs an edge,
but one is enough: the cluster starts with a single edge whose core is the
whole world, and splits it as load arrives. Equal-sized cores would keep
idle edges running over empty land.

Cells are laid out in **Hilbert curve order**, and each core is one
contiguous range of that order:

```
Hilbert order (4×4)        cores as ranges
 0  3 │ 4  5               [0, 5)  → A   top-left quadrant + 4
 1  2 │ 7  6               [5, 9)  → B
──────┼──────              [9, 16) → C
14 13 │ 8  9
15 12 │11 10
```

- Every quadtree block is a contiguous range, so a core made of aligned
  quadrants, as in a quadtree split, is just one kind of range.
- The layout is a sorted list of cut points. Finding a cell's edge is a
  binary search.
- **Split** adds a cut inside a range, **merge** removes the cut between two
  neighbors, whose union is always contiguous, and **rebalancing** shifts a
  cut by a few cells. Load moves a little at a time, rather than by halves
  or quarters.
- A world that is not a 2ⁿ×2ⁿ square is padded up to one. Padding cells
  hold nothing and cost nothing.

Coordinator picks where to cut:

- **Balance first.** Candidates lie near the point that halves the range's
  load, say within ±10%.
- **Then crossings.** Among the candidates, the cut with the least measured
  traffic across it wins, since every crossing is a reconnect. Wilderness,
  mountains and rivers tend to win on their own.
- **Then shape.** On a tie, a cut on a quadtree block boundary wins. Aligned
  blocks keep cores close to squares, and so halos small, while an arbitrary
  cut along the curve can leave a ragged edge.

The halo is derived from the core. It never depends on where shards are.
When sims split a crowded area into many shards, the edge covering it does
not change. It just has more active upstream sessions.

### Hotspots

When an edge saturates, a core can be served by **R edges**, with clients
spread among them. Replication ingress grows by R and per-edge fan-out
shrinks by R. Since fan-out dominates, that is usually worth it.

Raising R only moves the clients that are rebalanced onto the new edge. No
cut moves, and nobody else reconnects. So the two tools split the work by
how long a load lasts:

- **R for transient load**, such as raids and events. It is fast to add and
  to take back, and is used at any core size.
- **Cuts for lasting load**, a change in where players are over days.

Splitting stops at a minimum core size. Halving a core halves per-edge
fan-out, but each half needs its own halo. Once the halves' extra ingress
costs more than serving the whole core with R = 2, which happens when the
core is only a few cells across, R takes over.

### Scaling

- **Hysteresis.** The split threshold sits above the merge threshold, and a
  layout change holds for a minimum time before the next one.
- **Ahead of the peak.** Load follows a daily pattern, so coordinator
  splits on forecast load before the peak and merges after it, rather than
  moving clients at the busiest hour.
- **Gradual handover.** After a cut moves, the gaining edge replicates the
  cells first. Entry then sends new logins there, and the edge that lost the
  cells moves its clients across in batches, each by
  [make-before-break](#moving-between-edges). The losing edge keeps
  replicating its old core and halo until its last client there has moved,
  so nobody's view develops a hole.

### Moving between edges

A client walking from A's core into B's moves **make-before-break**, while
it is inside both edges' replicas:

1. Edge A issues the client a [token](#tokens) for edge B.
2. The client connects to B. B already replicates the area and syncs it
   right away.
3. Once B is live, the client sends inputs to B only, B claims
   [presence](#presence), and the client drops A. Client-addressed messages
   that still reach A in between are forwarded to B.

A's replicas cover the client until it is a view radius from their far
side. Past A's core, that leaves the **handover margin**, the cell side
minus the view radius, to finish the move in. The margin has to cover the
fastest movement over the time a move takes.

Teleports, respawns and instance changes jump past any overlap. They move
**break-then-make**: the client gets a token, drops A, and connects to B
behind a loading screen.

### Shared cell frames

An edge encodes each cell's client frame **once per tick**, and every
client whose view covers the cell gets the same bytes. A crowd watching the
same cells costs the edge one encoding, plus socket writes.

- **Interest is per cell.** A client receives whole cells rather than a
  precise radius. A reduced variant of a frame, at a lower rate or with
  fewer components, can serve clients that see a cell from afar.
- **Private data goes per client.** A character's own inventory or anything
  only one client may see bypasses cell frames. Team-only visibility splits
  a cell's frame by visibility class.
- **Every client of a frame shares its baseline.** Cell frames are
  reliable-ordered deltas. A client whose view gains a cell first receives
  the cell's snapshot, which is also encoded once per tick and shared. When
  the edge itself [resyncs](#resync-and-backpressure) a cell, every client
  watching it gets the new snapshot.

### Inside an edge

> **Needs discussion.** How an edge's threads split the work is not
> settled.

The questions, whatever the model:

- **Who owns a cell's replica** and encodes its frames: one thread per cell,
  a range of cells per thread, or something else. Spreading cells by hash
  sends every thread a batch from every other thread each tick, up to
  64 × 64 = 4096 batches for 64 threads.
- **Where a client's session lives**, and whether clients land on any
  thread, as a `SO_REUSEPORT` listener would do, or by position.
- **How client-addressed messages reach the session's thread.** Hubs and
  sims send messages for one player, such as party updates and private
  data. Either presence and the player entity record the thread, or the
  receiving thread forwards inside the edge.
- **How a broadcast spreads**, such as world chat, received once per edge.
- **How a read** that spans several cells' replicas is answered.

One candidate is to split threads by role: each cell's replica is owned by
one thread, which encodes its frames, and sessions land on any thread and
collect the frames they need. Only immutable `Bytes` cross threads, and a
crowd's socket writes spread over all threads. The questions above are what
it leaves open.

## Sims

### Replicas between shards

Each sim shard replicates the halo of its cells: the adjacent cells owned by
other shards. These replicas use the same per-cell streams as edges.

- Entities in a replica are read-only. Acting on one, such as dealing
  damage, sends a message to its owner, which applies it on its next tick.
- Since nothing reaches past one cell, a shard never needs more than its
  halo to see everything its entities can see or touch.

### Entity migration

A player entity's owner is the owner of the cell it stands in. When an
entity crosses into a cell owned by another shard S′, S hands it over:

1. Once the entity is past the cell boundary by a small margin, so that
   jitter on the line does not bounce it, S removes it and sends
   `Migrate { uid, state, last_input_seq }` to S′.
2. S emits `Leave { uid, to }` on the old cell's stream. S′ adopts the
   `UniversalId` and emits `Enter` on the new cell's stream.
3. The two streams come from different shards, so a replica may see `Leave`
   before `Enter`. It keeps the entity, frozen, until `Enter` arrives or a
   short timeout passes, so the entity neither flickers nor shows twice.
4. Inputs that reach S after the move are forwarded to S′ for a while.

Within one shard, crossing a cell boundary moves nothing.

## Entry

Edges are spatial, so a client cannot pick one on its own. Entry does it:

1. The client reaches entry at one public name, through an ordinary load
   balancer.
2. Entry authenticates the account and lists its characters. Password
   hashing and account lookups are blocking work, which stays off the pinned
   threads' rings.
3. On character select, entry reads [presence](#presence) as a hint only.
4. Entry reads the character's last position, maps it to a cell, picks the
   least loaded edge of that cell's set, by the load that coordinator
   reports, and returns the edge's address and a token.
5. The client connects to the edge. The edge verifies the token and claims
   presence. Once the claim is granted, it asks the cell's owner shard to
   load and spawn the character. If the cell is not in its core, because
   the layout changed in between, the edge hands the client on as in
   [Moving between edges](#moving-between-edges).

Entry also runs the login queue when the world is full, and rejects
incompatible client versions. It keeps no session state, so it scales by
adding instances behind the load balancer.

A client that drops reconnects through entry. Short reconnects can go
straight back to the same edge with a resume token, within a grace period.

### Tokens

A token names the account, the character, the target edge, an expiry and a
one-time nonce, and is signed. Edges verify it locally, with no round trip,
and remember spent nonces until they expire. Entry and edges issue the same
format, so a first connection and a move between edges are the same path on
the receiving edge.

## Hubs

### Keys and slots

Every hub object is keyed by a **natural key**, `(kind, id)`: the account
for presence, the character for a social profile, the guild, the party, the
chat channel. Its placement is

```
slot = hash(kind, id) mod SLOTS   # SLOTS a power of two, fixed for the cluster's lifetime
shard = slot_map[slot]            # owned by coordinator
```

Whoever asks for a new object mints its id, such as a random 64-bit number,
and sends the creation to the slot's owner, which creates it there. Hub
objects do not use `UniversalId`, which names its minting shard and would
place objects away from their slot.

The hash maps keys to a fixed number of slots, never directly to shards:

- `hash mod shards` reshuffles almost every key when a shard is added.
- Consistent hashing moves few keys, but balances poorly without many
  virtual nodes, and gives no handle to move one hot range on purpose.
- Fixed slots, far more of them than shards (thousands), move one at a time,
  and the map is small enough to push to every edge, sim and entry.

A slot is to a hub what a cell is to a sim. Slots move between hub shards
with the same protocol as [cell handoff](#cell-handoff): `(epoch, seq)` per
slot, a `Handoff` marker, and forwarding by the old owner. The slot's
idempotency records move with it. Scaling out means adding a hub node and
moving slots onto it.

### Requests

- **Client → hub**: the client sends to its edge. The edge hashes the key,
  looks up the slot's owner, and forwards over its session.
- **Sim → hub**: gameplay that writes hub state, such as guild contribution
  from a kill, goes directly from the sim shard to the hub shard.
- **Idempotency**: a sender that times out or loses its session retries, so
  every write from an edge or a sim carries an idempotency key, and the slot
  remembers the keys it applied.
- **Hub → players**: a hub never connects to clients. It looks up its
  members' edges in [presence](#presence), sends one message per edge, and
  the edges fan out. A party of five on three edges costs three messages. A world chat
  message goes once to every edge.
- **Hub → sim**: a message from a hub to a character goes over the
  sim → hub session to the character's shard, not through its edge.
  Messages that reach the old shard after a migration are forwarded, as
  inputs are.
- **Hub cache**: when gameplay needs hub state, such as a character's party
  for shared experience, the sim shard reads it from a cache of hub objects
  by kind and key. A miss starts a fetch, and sims never query a hub
  synchronously during a tick. The shard holds a lease on each entry, and
  the hub pushes every change of the object to it until the lease ends.
  Every copy carries its object's version, and the shard ignores older
  ones. Entries do not migrate with entities.

### Ownership rules

- **One writer per object.** The party owns its member list. Sims read it
  from their hub cache, which the party's pushes keep current.
- **No cross-slot transactions.** An operation that touches two objects gets
  an object of its own to own it: a trade holds both sides' items in escrow
  and settles in steps.
- **Hot objects do not split.** A slot is the smallest unit that moves, so
  an object that outgrows one shard is handled by its shape: world chat keeps
  no member list and broadcasts to edges, and widely read objects can be
  replicated to edges like cells.

### Presence

Presence is a hub object per account, and acts as the account's lease: the
character in play, the edge serving it, and a generation number.

- **The edge claims it** when a client connects, with the token's account.
  The hub serializes claims, so two entries reading "offline" at once cannot
  both win.
- **The later login wins.** A claim on an account that is already online
  bumps the generation and tells the old edge to drop its session.
- **The new character waits for the old one.** The hub grants the claim
  once the old session's character is saved and despawned, or once the
  lease times out because the old edge is gone. Only then does the new edge
  spawn the character, so it never loads a stale save.
- Edges update presence on every move between edges. Hubs use it to reach
  players.

## Sessions between nodes

| From | To | For |
|---|---|---|
| edge | every sim shard | subscriptions, inputs, reads |
| edge | every hub shard | client requests, presence |
| entry | hub shards | presence |
| sim shard | neighbor sim shards | halo replicas, cell handoff, entity migration, forwarding |
| sim shard | hub shards | writes, hub cache, hub → sim messages |
| hub shard | hub shards | slot handoff, forwarding |
| coordinator | every node | maps, decisions |
| every node | coordinator | load, crossing traffic |

Shards on the same node talk over in-process channels
(`crates/uju/src/runtime/node.rs`), and over TCP across nodes.

**Edges and entry keep full meshes** to shards, opened once and kept. Only
subscriptions are selective. For example, with 640 sim shards and 64 edge
connection points, each sim shard holds 64 edge sessions and each
connection point 640, mostly idle. When a cell or slot moves to a shard a
peer has not used
yet, the session is already there. If the cluster outgrows this, switch to
connecting on demand. Whether an edge connects per thread or per node
depends on [Inside an edge](#inside-an-edge).

**Shards connect to each other on demand.** Each needs only its neighbors
and the targets of its handoffs, which coordinator knows in advance, since
it decides every move. Coordinator names the peer in its decision, the
shards connect, and only then does the move start. Idle shard-to-shard
sessions close after a while.

See [Network](../operations/network.md) for how each session's packets land
on the right CPU.

## Membership

- **Join**: a node registers with coordinator, which adds it to the
  directory. Edges, entry and the shards that coordinator names connect to
  it. Once they report connected, coordinator may place cells, slots or an
  edge cut on it.
- **Leave**: coordinator drains the node first: its cells and slots move
  away, or its edge's clients move to other edges. Then coordinator removes
  it from the directory and peers close their sessions.

## Replication

Each sim shard streams deltas **per cell** to that cell's subscribers: the
edges whose core or halo contains it, and the sim shards whose halo does.
The subscriber set is part of the cell's state and moves with it.

Every cell carries `(epoch, seq)`. The epoch increments on each ownership
change.

> **TODO.** Deltas name components by the static ids the protocol compiler
> assigns (see [Components](../protocol/schema.md#components)). The ids
> `uju::ecs` assigns at startup differ between binaries that link different
> components, such as a sim and an edge.

### Cell handoff

Coordinator decides that cell `c` moves from shard S to shard S′. Then:

1. S sends `c`'s last deltas, then `Handoff { cell: c, epoch: e, to: S′ }`,
   in order on the same stream.
2. S′ takes over `c`'s state and subscriber set, and streams epoch `e + 1`.
3. S and S′ use different sessions, so a subscriber may see `e + 1` before
   `Handoff(e)`. It buffers each epoch until the previous one's handoff
   arrives, `e + 2` behind `e + 1` as well. It already holds everything up
   to `e`, so no snapshot is needed.

### Who owns a cell

- **For a subscribed cell, the stream** is the truth: a subscriber learns
  every change from `Handoff`, in order with the data.
- **For any other cell, coordinator's map** is, and it may be stale. A
  shard that receives a subscription, snapshot request or input for a cell
  it no longer owns forwards it to the owner it handed the cell to.

### Resync and backpressure

Every queue is bounded in bytes: a shard's per-subscriber queues, and an
edge's per-client queues. The TCP egress queue in
`crates/uju/src/net/transport/tcp/session.rs` is unbounded today.

- **Sims never wait.** When a subscriber's queue overflows, the shard drops
  that subscriber's streams and tells it. The subscriber requests a snapshot
  of each cell and resumes from it. The same path covers dropped sessions
  and restarts.
- **Edges never wait on clients.** Shared frames cannot skip a delta for one
  client, so when a client's queue overflows, the edge drops its frames and
  resends snapshots of its cells. A client that keeps overflowing is
  disconnected.

## Inputs

- A client's inputs go to the owner of the cell its character stands in.
  The edge knows it from the cell's stream, through `Handoff` and
  `Enter`, so ownership and replicated data never disagree.
- Inputs carry a per-client sequence number. A cell handoff or an entity
  migration hands over the last applied number.
- An input that reaches an old owner is forwarded to the new one, so the
  new owner may receive input `i + 1` directly before `i` arrives through
  the old one. It holds later inputs until the gap fills, for a bounded
  time, then skips the gap.

## Reads at the edge

Requests that only read the replicated state are answered by the edge from
its replicas. The replicas lag the sim by about a tick plus transit, so
anything that needs authority, such as a purchase or a state change, goes
to the owning shard.

## Coordinator

- **cell → sim shard**: authoritative, changed only by coordinator's
  decisions.
- **slot → hub shard**: likewise.
- **cell → edge set**: cut points along the Hilbert order, and R per core.
  Halos are derived.
- **directory**: the address of every node, shard and edge.
- Collects shard and edge load and crossing traffic, and forecasts the daily
  pattern. Decides cell and slot moves, cuts and R (see [Layout](#layout)),
  and drains nodes (see [Membership](#membership)). Pushes maps to edges,
  sims, hubs and entry, and edge load to entry.

## Open questions

- **Edge threads**: see [Inside an edge](#inside-an-edge).
- **Cell size**: at least the view radius and interaction range plus the
  handover margin. Larger cells coarsen interest and balancing.
- **Unreliable cell frames**: shared deltas assume a reliable-ordered
  stream. Unreliable delivery over RUDP needs per-cell keyframes.
- **Layout policy**: split and merge thresholds, the minimum core size,
  when to raise or lower R, and which clients move.
- **Sim layout**: cell → sim shard could use the same Hilbert ranges, so
  that shard regions stay contiguous and share less halo. Sims rebalance far
  more often, so the thresholds and step sizes would differ.
- **SLOTS**: which power of two, and which hash.
- **Persistence**: when shards load and write back their cells' and slots'
  objects, and where a character's last position is read from at entry.
- **Token keys**: every edge issues tokens, so every edge holds a signing
  key. How keys reach entries and edges, and how they rotate.
- **Failures**: how coordinator stays available, and what happens to a
  node's cells, slots and clients when it dies rather than leaves.
