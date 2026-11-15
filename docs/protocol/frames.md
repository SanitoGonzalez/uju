# Frames

How uju's runtime frames what its links carry: batches, cell frames,
snapshot chunks, owner frames and migrations. A message's body is laid out
as in [Wire Format](wire-format.md). This document covers what a message
cannot hold: a cell's delta easily outgrows a message's 65535 bytes, so
frames use `u32` lengths, and only the items inside them, such as a
component's encoding, stay schema objects.

> **Status.** Only the [sections](#sections) of `replace`, `patch` and
> `ops` components and of tags are implemented, by the ECS's
> [replicated tables](../ecs/change-tracking.md#sections). The ECS's
> [`Uid`](../ecs/entities.md#uid) is the 8-byte `incarnation << 32 | serial`
> this document assumes, but the schema still has the 12-byte `uentity`.

## Principles

- **Packed, unaligned, little-endian**, as in [Wire Format](wire-format.md).
- **Reliable and ordered.** Every link is a reliable-ordered stream, TCP
  today. Unreliable delivery is an [open question](#open-questions).
- **Encoded once, sliced after.** A sim encodes each cell's delta once per
  tick. Edges and neighbor shards take parts of it by slicing bytes, and
  never decode and re-encode it to forward it.
- **Lists are lengths, not counts.** A list of fixed-size items is a `u32`
  byte length followed by the items back to back, so a reader takes it as
  a slice of views. Its count is the length divided by the item size.
- **Entities are named by uid**, a `u64`, everywhere. There is no
  cell-local id.

## Streams

| Stream | From | To | Holds |
|---|---|---|---|
| Cell | A cell's owning sim shard | Edges whose core or halo holds the cell | The client sub-frames of each tick's delta |
| Halo | A cell's owning sim shard | Neighbor shards whose halo holds the cell | Every sub-frame, `internal` included, without the `@halo(false)` sections |
| Owner | An entity's owning sim shard | The edge serving the entity's client | That entity's `@visibility(owner)` components |
| Client | An edge | A client | The cell frames of the cells its view covers, and its own entity's owner frames |

A link can carry several streams, and messages besides. A sim → edge link
carries the cell streams of every cell the edge subscribes to, and the owner
streams of the edge's clients.

## Batches

Every link carries a sequence of **batches**:

```
batch  := length:u32 | header | item*
item   := kind:u8 | item body
```

`length` counts the header and the items. The header depends on the sender:

| Sender | Header |
|---|---|
| sim | `tick:u64 | tick_time:u64 | send_delay:u32` |
| edge, to a client | `send_time:u64` |
| any other | none |

- **Sims.** A sim sends at most one batch per link per tick, holding that
  tick only. `tick` counts the shard's ticks. `tick_time` is when the tick
  started, in microseconds on the shard's own monotonic clock, and
  `send_delay` is how long after it the batch was sent. Each tick, a sim
  sends a batch on every link that carries a cell stream, with no items if
  nothing changed, as a heartbeat.
- **Edges.** `send_time` is in microseconds on the edge's clock, counted
  from the start of the client's session.
- Items are applied in order. A receiver may cap a batch's length and drop
  a session that exceeds it.

Item kinds:

| Kind | Item | Links |
|---|---|---|
| `1` | [Message](#messages) | every link |
| `2` | [uju message](#ujus-messages) | every link |
| `3` | [Cell frame](#cell-frames) | sim → edge, sim → sim, edge → client |
| `4` | [Snapshot chunk](#snapshot-chunks) | sim → edge, sim → sim, edge → client |
| `5` | [Owner frame](#owner-frames) | sim → edge, edge → client |
| `6` | [Migration](#migrations) | sim → sim |
| `7` | [Time](#time) | edge → client |

A frame item is its kind, then its body's length, then its body:

```
frame item := kind:u8 | length:u32 | body
```

### Priority

A sender fills a batch with messages and deltas first, then with snapshot
chunks, up to a byte budget per link and tick it chooses. One large
snapshot so never holds up every other cell's deltas on the link, which
matters most on a client's link.

## Messages

A message item follows the [envelope](wire-format.md#envelope):

```
message := [role:u8] | id:u16 | [key] | [length:u16] | body
```

- `role` is present on a client's link to its edge only, where several
  role pairs share the link. It names the destination upstream and the
  source downstream: `1` edge, `2` sim, `3` hub.
- `id` is the message's id in its [role pair](wire-format.md#message-ids).
- `key` is present for a message to a hub: the object's key, encoded as
  the kind's key type.
- `length` is absent for a fixed-size message, whose id implies it.

## Cell frames

A cell frame holds one cell's delta for one tick:

```
cell frame        := cell:u32 | epoch:u32 | sub-frame*     (server links)
client cell frame := cell:u32 | sub-frame*                 (client links)
sub-frame         := id | length:u32 | body
```

Its tick is the batch's. A cell that did not change in a tick sends no
frame.

### Sub-frames

A cell's delta is split by who may see it:

```
id := audience:u16 | [key]
```

| `audience` | Sub-frame | Holds |
|---|---|---|
| `0` | `public` | `@visibility(public)` components and tags |
| an audience's [id](wire-format.md#audience-ids) | that audience and `key` | components and tags with that `@visibility`, of the entities holding `key` |
| `65535` | `internal` | the `@halo(true)` components and tags with `@visibility(none)` or `@visibility(owner)`, every entity's presence, and audience keys |

`key` follows only an audience's id, encoded as the audience's key type.
A cell frame holds a sub-frame per id present in the cell that tick, in
ascending `(audience, key)` bytes, so a receiver can binary search them.

Who receives which sub-frames:

- An **edge** receives every sub-frame but `internal`. It writes each
  client's cell frame as a header of its own followed by slices of the
  sub-frames the client's observer sets hold.
- A **neighbor shard** receives every sub-frame, `internal` included, with
  the sections of `@halo(false)` components cut out. The sim builds it by
  copying byte ranges, and rewrites only the sub-frames' lengths.
- `@visibility(owner)` components reach clients only through
  [owner frames](#owner-frames).

### Body

```
body     := presence:u8 | [gone] | [moved] | [entered] | [keys] | section*
gone     := length:u32 | uid*
moved    := length:u32 | (uid | to:u32)*
entered  := length:u32 | uid*
```

`presence` has a bit per list that follows: `0` gone, `1` moved, `2`
entered, `3` keys. A list that is absent is empty. The other bits are zero.

- **`gone`** lists the entities that left this sub-frame and went nowhere
  the receiver should wait for: despawned, or out of the sub-frame's
  audience after a key change.
- **`moved`** lists the entities that left for cell `to`. A receiver that
  stops holding one keeps it, frozen, until its `entered` arrives from that
  cell, or a short timeout passes, so it neither flickers nor shows twice.
  A client whose view does not cover `to` drops it at once.
- **`entered`** lists the entities that joined this sub-frame: spawned,
  arriving from another cell, or into the audience after a key change.
  Their values follow in the sections.
- **`keys`** is in `internal` only. See [Audience keys](#audience-keys).

An entity's presence travels in every sub-frame that holds any of its
components, never only in `public`, so a client outside a stealth entity's
audience never learns it exists. `internal` holds every entity of the
cell, so a neighbor shard's replica exists even without components it can
see.

### Sections

```
section := component:u16 | parts:u8 | part*
part    := length:u32 | item*
```

A section holds one component's changes. Sections come in ascending
[component id](wire-format.md#component-ids), at most one per component.
`parts` has a bit per part that follows, in this order:

| Bit | Part | Items |
|---|---|---|
| `0` | values | `uid | value`: the full encoding of each entity that entered, gained the component, or was resynced, and of each `replace` change. A tag's items are uids |
| `1` | changes | `uid | len | patch` for `patch`, `uid | len:u16 | ops` for `ops` |
| `2` | removed | `uid`: the entities that lost the component and stay |

- **Values.** A fixed-size component's items are packed, `uid` then
  value, so a fixed-size `replace` component's values read as a slice of
  views. A variable-size component's items are `uid | len | value`, where
  `len` is 1 byte when the component's largest encoding is at most 255
  bytes, and 2 otherwise, as a collection's
  [length prefix](wire-format.md#collections).
- **Patches.** `len` follows the same rule, applied to the
  [patch](wire-format.md#patches)'s largest encoding.
- **Ops.** `ops` is the entity's ops of the tick, in order, laid out as a
  `vec` of the op union with no `max_length`. An encoder whose ops for
  an entity would not fit `len`, a `u16`, sends the value instead.
- An entity appears at most once in a section. Within a tick, an add then a
  remove cancel out, a remove then an add is a value, and a resync drops
  the tick's ops.
- A `replace` component has no changes part. Its changes are values. A tag
  has values and removed only.

### Audience keys

The `keys` list of `internal` carries the audience keys that changed, so
gameplay on a neighbor shard reads a replica's key as it reads an owned
entity's:

```
keys      := length:u32 | audience*
audience  := id:u16 | set | cleared
set       := length:u32 | (uid | key)*
cleared   := length:u32 | uid*
```

Audiences come in ascending id. An entity that entered `internal` has its
keys in `set`. Observer sets do not travel on the halo stream.

### Applying a tick

A receiver applies a cell's sub-frames for a tick together, in this order:

1. every sub-frame's `gone` and `moved`,
2. every sub-frame's `entered`,
3. `keys`,
4. every section.

So a key change, a `gone` in the old sub-frame and an `entered` in the new
one, never shows as a gap.

**Bindings.** A receiver binds each `(uid, sub-frame id)` to the cell whose
`entered` it received last for them, and drops the binding on a `gone` or
`moved` from that cell. It skips sections, `gone` and `moved` naming that
uid from any other cell. A sub-frame's components go with its binding, and
the entity goes once nothing binds it. An entity moving between cells of
different shards can thus have its `entered` arrive before the old cell's
last updates and `moved`, which are ignored rather than applied over newer
state.

A section naming a uid that the sub-frame does not bind, from the same
cell, is an error.

### Example

```
namespace demo

component Health { hp: u32, max_hp: u32 }
tag Dead
```

Clients see both, so `demo.Dead` is component `1` and `demo.Health` is
`2`. In cell 42, epoch 3, entity `1:5` (incarnation `1`, serial `5`) spawns
with full health, `1:6` drops to 80 hp and `1:7` dies. The cell frame is
81 bytes:

```
2A 00 00 00                cell 42
03 00 00 00                epoch 3
00 00                      sub-frame: public
43 00 00 00                67 bytes
04                         presence: entered
08 00 00 00                entered: 8 bytes
05 00 00 00 01 00 00 00      1:5
01 00                      section: component 1, Dead
01                         parts: values
08 00 00 00                8 bytes
07 00 00 00 01 00 00 00      1:7
02 00                      section: component 2, Health
01                         parts: values
20 00 00 00                32 bytes, 2 items of 16
05 00 00 00 01 00 00 00      1:5
64 00 00 00 64 00 00 00      hp 100, max_hp 100
06 00 00 00 01 00 00 00      1:6
50 00 00 00 64 00 00 00      hp 80, max_hp 100
```

The edge forwards it to a client as `2A 00 00 00`, the cell, followed by
the same 73 bytes of the `public` sub-frame.

## Snapshot chunks

A snapshot is one sub-frame's full state at a tick. A subscriber receives
one per sub-frame on subscribing or resyncing, and a client on gaining a
sub-frame, whether its view gained the cell or its observer set gained a
key. A snapshot is split into chunks at entity boundaries:

```
chunk        := cell:u32 | epoch:u32 | tick:u64 | sub-frame id | flags:u8 | body   (server links)
client chunk := cell:u32 | time:u64 | sub-frame id | flags:u8 | body              (client links)
```

- Bit `0` of `flags` marks the last chunk. The other bits are zero.
- The body is a [sub-frame body](#body) with only `entered`, `keys` and
  values. Every entity a chunk enters has all its values for the sub-frame
  in that chunk.
- A chunk is at most a size the sender chooses, such as 16 KiB, unless one
  entity alone takes more.
- The receiver applies the chunks together, once the last one arrives. The
  sub-frame's state becomes the snapshot's: entities it does not list go,
  and components it does not give are removed. An edge applies it onto its
  `World` as a diff, rather than despawning and re-entering every entity.
- Meanwhile the receiver holds the cell's later frames for that
  sub-frame, and drops those at or before the snapshot's tick.
- A chunk for another `(epoch, tick)` of the same sub-frame drops the
  unfinished snapshot.
- A snapshot at tick `t` includes every op of tick `t`.

An edge builds its clients' snapshots from its `World`. A client chunk's
`time` is when the cell's latest applied tick happened, on the edge's
clock.

## Owner frames

An entity's `@visibility(owner)` components go straight from its owning
shard to the edge serving its client, and on to the client:

```
owner frame        := uid:u64 | owner epoch:u32 | flags:u8 | section*   (server links)
client owner frame := uid:u64 | flags:u8 | section*                     (client links)
```

- The sections are laid out as a cell frame's. Bit `0` of `flags` marks a
  full state: the receiver removes the owner components it does not give.
  The other bits are zero.
- An entity's first owner frame to an edge is a full state, which an edge
  move sends again.
- **Across migration.** When the entity moves from S to S′, their owner
  frames arrive over different sessions. S ends the entity's owner stream
  with an `OwnerHandoff` naming S′. The owner epoch travels with the entity,
  and S′ increments it. The edge holds owner frames of a newer epoch until
  the old epoch's `OwnerHandoff` arrives. Nothing is resent.
- The edge strips the owner epoch before forwarding, and forwards the frame
  under the [time](#time) of its tick.

## Migrations

An entity moving to a cell another shard owns goes in a migration item, on
the S → S′ link:

```
migration := uid:u64 | cell:u32 | flags:u8 | section*
```

- `cell` is the cell the entity enters on S′.
- Bit `0` of `flags` marks a full migration, sent when S′ does not
  replicate the entity's cell, as on a teleport. The sections then hold
  every component's value, and S′ creates the entity.
- Otherwise S′ promotes its replica, and the sections hold only what the
  replica lacks: the `@halo(false)` components' values, and the entity's
  changes to `@halo(true)` components in its last tick on S, which the halo
  stream never carries, since the entity leaves. The halo stream and the
  migration share the S → S′ link, so the replica is at the tick before
  when the migration arrives.
- Runtime state that travels with the entity, such as its last input
  sequence, its owner epoch and its transactions, is a component of uju's
  own schema, so it travels as sections too.

A cell handoff sends a migration item for each of the cell's entities, then
`Handoff`. A shard that already replicates the cell receives only what its
replicas lack, as above.

A migration is a frame rather than a message, since a full one can outgrow
65535 bytes.

## Time

Each shard ticks on its own clock, at the rate the cluster configures, so
tick numbers mean nothing across shards. Clients interpolate by time
instead.

**Server links.** A cell's frames are ordered by `(epoch, tick)`. Ticks
restart from another shard's counter after a handoff, but the epoch has
grown. An edge maps each sim's clock onto its own: per session, it keeps
the smallest `arrival - (tick_time + send_delay)` over a sliding window, and
adds it to tick times. That is accurate to the datacenter's minimum latency,
follows clock drift, and needs no NTP.

**Client links.** The edge strips epochs and ticks, which clients do not
need, since the edge places snapshots in its stream itself. A time item
gives the time of the frames after it:

```
time := time:u64
```

It is in microseconds on the edge's clock, counted from the start of the
session. The cell and owner frames after a time item, up to the next one,
come from one source tick, and a cell has at most one cell frame under a
time item. The client maps the edge's clock onto its own as the edge maps a
sim's, from batch headers' `send_time`, and learns the mapping again when it
moves to another edge.

An entity crossing between shards jumps by its speed times the mapping
error, which is small. That is accepted.

## uju's messages

What uju's processes exchange outside frames is declared in a schema of
uju's own and laid out as any message. They travel as item kind `2`, so
their ids never mix with the user's.

| Message | From → to | Says |
|---|---|---|
| `Subscribe` | edge or sim → sim | Start a cell's stream, with a snapshot of each sub-frame. A neighbor shard asks for the halo stream |
| `Unsubscribe` | edge or sim → sim | Stop a cell's stream |
| `Dropped` | sim → edge or sim | The receiver's queue overflowed and its streams were dropped. It subscribes again |
| `Handoff` | sim → subscribers and the new owner | A cell's last frame of an epoch, and the shard that streams the next |
| `OwnerHandoff` | sim → edge | The last owner frames of some entities, and the shard that sends the next |
| `Resync` | client → edge | The client failed to apply a frame and wants its cells' snapshots again |

`Handoff` follows the cell's last frame on the same stream. A subscriber
that receives frames of epoch `e + 1` first holds them until `Handoff(e)`
arrives, `e + 2` behind `e + 1` as well.

## Errors

A receiver that finds a frame invalid drops the stream it belongs to and
resyncs: a server subscribes again, an edge resends its client snapshots,
and a client sends `Resync`. A frame is invalid when:

- a length runs past its container, or a packed list's length is not a
  multiple of its item size;
- `presence`, `parts` or `flags` set an unused bit;
- sections are not in ascending component id, or one names a component
  the sub-frame may not hold, such as a `@visibility(none)` one outside
  `internal`;
- an entity appears twice in a section, or a section names a uid the
  sub-frame does not bind, from the same cell;
- a value, patch or op fails [validation](wire-format.md#validation);
- an `ops` component's `apply` fails.

## Open questions

- **uju's schema.** Its endpoints, such as `edge -> sim`, and whether its
  generated code is checked in or built by `build.rs`, are open in
  `components-plan.md`.
- **Cell ids.** `u32` assumes a grid of at most 2³² cells per world, and
  says nothing of instances.
- **Unreliable cell frames.** Shared deltas assume a reliable-ordered
  stream. Unreliable delivery over RUDP needs per-cell keyframes.
- **Compression.** Dense `u16` entity ids inside a frame, or a presence
  bitmap when most of a cell changes, if measured traffic calls for them.
