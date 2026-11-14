# Schema

> **Status.** Only attributes, naming, consts, types, unions, foreign types,
> messages, hub kinds, and components are written up here. Until
> the rest of the language is, `crates/uju-protoc/tests/data/example1.uju`
> shows every declaration kind, type and constraint.

## Declarations

Declarations, `namespace`, `use`, `const` and `type` included, take no
terminator. Each starts with a keyword or an [attribute](#attributes),
which ends the one before it, so `type A = u8 type B = u16` declares two
types. A `;` is an error.

## Attributes

Attributes go above the declaration they describe, each as `@name` or
`@name(args)`:

```
@replicate(ops(InventoryOp))
@visibility(owner)
component Inventory {
    items: vec<Item>[max_length=64],
}
```

- An argument is a literal (`1000`, `-0.5`, `"text"`, `true`, a
  [duration](#durations) such as `1000ms`, a
  [timestamp](#timestamps) such as `2026-11-07T12:00:00Z`), a path
  (`Gold`, `demo.items.Gold`), or a call of a path with arguments
  (`ops(InventoryOp)`). Arguments are separated by commas, and a trailing
  comma is allowed.
- Parentheses hold at least one argument: write `@large`, not `@large()`.
- One `@` holds one attribute. Lists go inside its parentheses, as in
  `@lock(Inventory, Gold)`. The same attribute given twice on a declaration
  is an error.
- Line breaks carry no meaning, but by convention each attribute takes a
  line of its own.
- Attributes go on `const`, `type`, `enum`, `union`, `struct`, `message`,
  `hub`, `audience`, `component` and `tag` declarations, at the top of a
  namespace or nested. They are not allowed on `namespace`, `use`, fields
  or variants, and one with no declaration after it is an error.

Constraints keep their brackets after the type, as in `u8[max=10]`. They
belong to the type, while attributes describe the declaration.

Three attributes are defined, all for [components](#components):

| Attribute | Goes on | Default |
|---|---|---|
| [`@replicate`](#replication-modes) | `component` | `replace` |
| [`@visibility`](#visibility) | `component`, `tag` | `public` |
| [`@halo`](#halo) | `component`, `tag` | `true`, but `false` with `@visibility(none)` |

One of them on any other declaration is an error naming where it goes,
such as "`@replicate` goes only on a component". `ujuc` reports any other
attribute as unknown.

## Naming

Names are ASCII: a letter or `_`, then letters, digits and `_`. Each kind of
name has a convention:

| Name | Convention | Example |
|---|---|---|
| Namespace segment | `snake_case` | `namespace demo.character` |
| Enum, struct, message | `PascalCase` | `message(sim -> client) SpawnPlayer` |
| Hub kind | `PascalCase` | `hub(GuildId) Guild` |
| Component, tag, audience | `PascalCase` | `component Health`, `tag Dead`, `audience Faction` |
| Enum variant | `PascalCase` | `Blue` |
| Union | `PascalCase` | `union InventoryOp` |
| Union variant | `PascalCase` | `Remove(u8)` |
| Type | `PascalCase` | `type PartyId = u64` |
| Const | `PascalCase` | `const MaxHp: u32 = 100` |
| Field | `snake_case` | `max_hp: u32` |
| Union variant field | `snake_case` | `Add { slot_index: u8 }` |

`ujuc` warns about a name that breaks its convention and suggests one that
follows it. The schema still compiles.

```
demo.uju:3:8: warning: struct `player_state` should be PascalCase, like `PlayerState`
```

### Why it matters

Backends rename schema names into their language's conventions, and the
conversion is only predictable from a known starting point. The Rust
backend, for instance, puts the types nested in `HTTPDetail` in a module
`http_detail`. A future C# backend would turn the field `max_hp` into a
property `MaxHp`.

Conversion splits a name into words:

- at each `_`
- before a capital that follows a lowercase letter or a digit
- before the last capital of an acronym followed by lowercase

Digits stay with the word before them. So `HTTPDetail` is `HTTP Detail`,
`Vec2D` is `Vec2 D`, and `player2_id` is `player2 id`. PascalCase keeps an
acronym as written, so `HTTPDetail` and `IOError` already follow the
convention.

Consts are PascalCase rather than `SCREAMING_SNAKE_CASE` because only that
direction converts without loss. `MaxHp` and `HTTPPort` become `MAX_HP` and
`HTTP_PORT` in Rust. `MAX_HP` has no case left to recover words from, so it
could only become `MAXHP` or, lowercasing every word, `MaxHp`, which turns
`HTTP_PORT` into `HttpPort`. C# and Unreal both name constants in
PascalCase.

### Collisions

Different names can convert to the same generated name. `HTTPDetail` and
`HttpDetail` both become the module `http_detail`, and two messages `M` in
namespaces `aB.c` and `a.bC` both become the dispatch variant `ABCM`. A
backend reports every collision as an error and generates nothing.
Following the convention makes collisions rare but doesn't rule them out.

## Consts

A const names a value both peers agree on, such as a limit a client checks
before sending:

```
namespace demo.chat

const MaxNameLength: u16 = 16
const TickRate: f32 = 30.0
const PvpEnabled: bool = true
const Greeting: string = "Welcome!\n"
const RespawnDelay: duration = 30s
const SeasonStart: timestamp = 2026-11-07T12:00:00Z

message(client -> sim) SetName {
    const MinLength: u8 = 2
    name: string[max_length=16],
}
```

- The type is an integer (`i8` to `i64`, `u8` to `u64`), a float (`f32`,
  `f64`), `bool`, `string`, `duration`, or `timestamp`, without constraints.
- An integer must fit its type. A float takes an integer or a float literal
  (`2` or `2.0`) and must be finite in its type. NaN and infinities can't be
  written.
- A string takes the escapes `\"`, `\\`, `\n`, `\r`, `\t`, `\0`, and
  `\u{1F600}` (one to six hex digits), and can hold any UTF-8. Its length is
  not capped, since it never goes on the wire.
- `true` and `false` are only literals where a value is expected, so they
  stay usable as names.
- A `duration` takes a [duration literal](#durations) and a `timestamp` a
  [timestamp literal](#timestamps). Each takes nothing else, so
  `const T: duration = 1000` is an error, and an integer or float type
  rejects both.

A const goes at the top of a namespace or in a struct or message, like a
nested type. Consts and types share each scope, so a const can't have the
name of a type declared beside it, and a type path that reaches a const is
an error. Consts never go on the wire, but they count toward both
[fingerprints](wire-format.md#fingerprint), by value rather than by
spelling: `1000ms` and `1s` are the same const, as are
`2026-11-07T12:00:00.000Z` and `2026-11-07T12:00:00Z`.

The Rust backend emits `pub const MAX_NAME_LENGTH: u16 = 16;` in the
namespace's module, and a nested const in its parent's module, as
`set_name::MIN_LENGTH`. A string is a `&str`.

A `duration` is a `::std::time::Duration`, still a `const`:

```rust
pub const RESPAWN_DELAY: ::std::time::Duration =
    ::std::time::Duration::from_micros(30000000);
```

`SystemTime` has no `const` constructor, so a `timestamp` is a `static`
computed on first use:

```rust
pub static SEASON_START: ::std::sync::LazyLock<::std::time::SystemTime> =
    ::std::sync::LazyLock::new(|| {
        ::std::time::UNIX_EPOCH + ::std::time::Duration::from_micros(1794052800000000)
    });
```

> **TODO.** Constraints and array lengths can't refer to consts yet, as in
> `string[max_length=MaxNameLength]` or `array<u32, SlotCount>`.

### Durations

A duration literal is an integer with its unit attached, such as `1000ms`,
`30s` or `5m`, so a name never carries its unit, as `timeout_ms` would.

- The units are `us`, `ms`, `s`, `m` and `h`. There is no `ns`, since a
  `duration` counts microseconds on the wire, and no `d`, since a day's
  length is ambiguous.
- The unit follows the integer with no space: `30s`, not `30 s`.
- Only integers: `1500ms`, not `1.5s`, so nothing rounds.
- One unit per literal: `90m`, not `1h30m`.
- A `duration` requires a unit, so `1000` alone is an integer, not a
  duration.
- The value goes from `0` to 10000 years. A negative or larger one is an
  error.

### Timestamps

A timestamp literal is a UTC date and time in RFC 3339, such as
`2026-11-07T12:00:00Z`.

- It ends in `Z`. Offsets such as `+02:00` are errors, and `T` and `Z` are
  uppercase.
- Seconds may take a fraction of exactly 3 or 6 digits, milliseconds or
  microseconds: `2026-11-07T12:00:00.250Z`, `2026-11-07T12:00:00.250125Z`.
- The date must exist, and the year goes from 1970 to 9999, since a
  `timestamp` counts microseconds since the Unix epoch on the wire.
- There are no leap seconds: seconds go up to `59`.


## Types

A `type` gives a scalar a name of its own, so that an id or a key can't be
mixed up with another value of the same width:

```
namespace demo.party

type PartyId = u64
type Level = u8[max=100]

struct Member {
    party: PartyId,
    level: Level,
}
```

- The target is a scalar: an integer, a float, `bool`, `timestamp`,
  `duration`, `entity`, `uentity`, `uuid`, an enum, or another `type`.
  Strings, `bytes`, arrays, collections, structs and unions are rejected.
- The target can't be optional. Make the field holding the type optional
  instead, as in `leader: option<PartyId>`.
- The target can carry constraints, as `Level` does. They belong to the
  type, so every `Level` is checked and [narrowed](wire-format.md#integers)
  the same way.
- Like any named type, a `type` takes no constraints where it is used:
  `Level[max=50]` is an error. Declare another `type` instead.

A `type` goes at the top of a namespace or in a struct or message, like an
enum. It can't refer to itself, directly or through other types. On the
wire it is [encoded as its target](wire-format.md#types).

The Rust backend emits a newtype, `pub struct PartyId(pub u64);`, and views
read a `PartyId` wherever one is stored.

## Unions

A union holds one of several variants, each of which may carry data. A
variant takes one of Rust's three forms:

```
namespace demo.inventory

union InventoryOp {
    Add { item: u32, slot: u8 },  // fields
    Remove(u8),                   // a single type
    Clear,                        // nothing
}

message(client -> sim) ApplyOps {
    ops: vec<InventoryOp>[max_length=8],
}
```

- Fields are written as in a struct and may be optional.
- A single type is any type a field can have, except `option<T>`.
- A variant without data has one spelling: `Clear {}` is an error that
  suggests `Clear`.
- A union needs at least one variant and holds at most 65535. Variant names
  are unique within the union.

A union goes at the top of a namespace or in a struct or message. It holds
no nested declarations of its own. A union is always variable-size, so it
can't be an array item, a set item, or a map key, and like any type it
can't refer to itself. See [Unions](wire-format.md#unions) for its
encoding.

The Rust backend emits an enum of the same shape, `InventoryOp`, and a view
enum, `InventoryOpView`, whose variants hold views of their data. The views
of fields variants go in a module named after the union, as
`inventory_op::AddView`.

`type` and `union` are keywords only where a declaration starts, as `true`
and `false` are only literals where a value is expected. They stay usable
as names, so `type: u8` is still a valid field.

## Foreign types

A backend can store a type of the user's in place of a declaration's owned
type, such as `::glam::Vec3` in place of a struct `uju.math.Vec3`. The
schema still fixes the wire layout: the foreign type converts to the
generated one to encode, and from it after decoding.

Mappings go in `ujuc.toml`, which holds `ujuc`'s options:

```toml
[rust.map]
"uju.math.Vec3" = "::glam::Vec3"
"demo.party.PartyId" = "crate::party::PartyId"
```

- `ujuc` reads `./ujuc.toml` when it exists. `--config <PATH>`, on both
  `ujuc protocol` and `ujuc dispatch`, reads another file instead, which
  must exist.
- `[rust.map]` maps a declaration's qualified path to a Rust type path. A
  nested declaration is named by its full path, as `demo.guild.Guild.Rank`.
- Only a struct, an enum, a union or a `type` can be mapped. Mapping a
  message, a hub kind, a component, a tag or an audience is an error, and so
  is a path the schema doesn't declare.
- An unknown section or key, an empty path or an empty type is an error.
- Mappings are not part of the protocol. Ids, the
  [fingerprints](wire-format.md#fingerprint) and the [IR JSON](ir.md) stay
  the same, and each binary may map differently. Keeping them consistent is
  the user's job.

The Rust backend still emits the mapped declaration's owned type and view.
Everything else that names the owned type stores the foreign one instead:
fields, the items and keys of `option`, `vec`, `set`, `map` and `array`,
union variants, `type` targets, and the `Key` of `HubKind` and `ToHub`.
Views still read the generated types. The user writes `From` both ways:

```rust
// `Vec3` is the generated type of `uju.math.Vec3`.
impl From<Vec3> for glam::Vec3 {
    fn from(v: Vec3) -> Self {
        glam::Vec3::new(v.x, v.y, v.z)
    }
}

impl From<glam::Vec3> for Vec3 {
    fn from(v: glam::Vec3) -> Self {
        Vec3 { x: v.x, y: v.y, z: v.z }
    }
}
```

- The generated type is local to the user's crate, so orphan rules allow
  both impls.
- Decoding builds the generated type and converts it. Encoding clones the
  foreign value and converts it every time, and a variable-size type
  converts twice, once to measure and once to write. Mappings suit small
  types, such as vectors and ids.
- The types holding a foreign type derive what the generated one does, so
  the foreign type implements those traits too: `Debug`, `Clone` and
  `PartialEq`, then `Copy`, `Default`, `Eq` and `Hash` where the generated
  type derives them, and `PartialOrd` and `Ord` when a `type` holds it. A
  set item or a map key needs `Eq` and `Hash`. rustc reports what is
  missing.

## Messages

A message names its endpoints in parentheses: the roles sending it, an
arrow, and the roles receiving it. Every message names them.

```
namespace demo.party

type PartyId = u64

hub(PartyId) Party {
    leader: u64,
}

message(client -> sim) Move { x: f32, y: f32 }
message(client -> hub<Party>) JoinParty {}
message(sim | hub -> client) Kicked { reason: string[max_length=64] }
message(sim -> sim) Hit { damage: u32 }
message(client <-> entry) Ping {}
```

- The roles are `client`, `entry`, `edge`, `sim` and `hub`, the processes
  of a [cluster](../architectures/topology.md#roles). Director speaks only
  uju's own messages, so `director` is not an endpoint.
- `->` goes one way and `<->` both ways. `|` lists the roles of one side,
  each once.
- A message travels every pair of a role on its left and a role on its
  right, and with `<->` every pair the other way too. So
  `sim | hub -> client` travels `sim -> client` and `hub -> client`. A pair
  written twice counts once: `sim <-> sim` is the one pair `sim -> sim`.
- Role names are not keywords, so `sim: u8` is still a valid field.

Two kinds of pairs are errors:

- `client -> client`. Clients reach each other only through servers.
- A pair joining `edge` to any role but `client`, such as `edge -> sim` or
  `hub -> edge`. Edges run no gameplay, so their only peer in the schema is
  the client, for what the connection itself needs, such as a ping. What
  edges exchange with sims and hubs, such as subscriptions, is uju's own.

### Relays

Clients connect only to entry and edges. A message between a client and a
sim or a hub is relayed by the client's edge, which reads its
[envelope](wire-format.md#envelope) and never its body. The schema still
writes it as `client -> sim` or `hub -> client`, with no `edge` between.

### Hub endpoints

`hub<Kind>` names a [hub kind](#hub-kinds), as in `hub<Party>`. The kind
resolves as a type path does, from the message's namespace and its `use`s.

- **Receiving.** A message to a hub names exactly one kind. Its id then
  implies the kind, and its envelope carries only the object's key. Bare
  `hub` on a receiving side is an error, since nothing could route the
  message, and so are `hub<Party> | hub<Guild>` and
  `hub<Party> <-> hub<Guild>`. With `<->`, both sides receive, so both
  follow this rule.
- **Sending.** Bare `hub` means any kind may send the message, and
  `hub<Party> | hub<Guild>` means only those kinds. `hub` beside
  `hub<Party>` on one side is an error. The restriction counts toward the
  [fingerprints](wire-format.md#fingerprint) and shows in the
  [IR JSON](ir.md#structs-and-messages), but generated code does not
  enforce it yet.

### Generation by role

`ujuc protocol rust --role sim` generates the code of one role: encoders
for the messages it sends, views for the ones it receives, and the
declarations those reach. A message the role neither sends nor receives is
left out, and so is what only such messages reach. Declarations no message
reaches are generated for every role, with both encoders and views, except
[hub kinds](#hub-kinds). Without `--role`, every role is generated.

The Rust backend emits a message as it does a struct, as an owned type and
a view. The root module holds what changes with any message:

- A `Link<From, To>` impl per message and pair, as
  `impl Link<role::Client, role::Sim> for MyRequest`, whose `ID` is its id
  in that pair. A role gets the impls of the pairs it sends or receives.
- A dispatch enum per pair the role receives, as `ClientToSimMessage<'a>`,
  whose `decode(id, buf)` decodes a message by its id.
- The [fingerprints](wire-format.md#fingerprint): `CLIENT_FINGERPRINT`, and
  `INTERNAL_FINGERPRINT` for every role but `client`.
- For an edge, a relay table per pair it relays, `client -> sim`,
  `client -> hub`, `sim -> client` and `hub -> client`, named after the
  pair, as `CLIENT_TO_HUB_RELAYS`. Each is a `&[Relay]` indexed by message
  id, which `Relay::find(table, id)` looks up. A `Relay` gives the
  message's size when fixed, and for a hub receiver a `HubRoute` with the
  kind's id and the key's size. That is all an edge needs to frame and
  route what it relays, so it gets no types for relayed messages.

A message to a hub also gets, beside it in its namespace's module, a
`ToHub` impl in every role that sends it. Its `Key` is the kind's key
type, so a sender routes it by a key of the right type. Where the kind
itself is generated, a `HubMessage` impl also names the kind, so a sim
finds the object's slot from the kind's id and the key.

## Hub kinds

A hub kind declares the state of one kind of
[hub](../architectures/topology.md#hubs) object, and the key that finds an
object:

```
namespace demo.guild

type GuildId = u64

hub(GuildId) Guild {
    name: string[max_length=24],
    level: u8,
    gold: u64,
}
```

- The key goes in parentheses after `hub`. It is fixed-size, holds no
  float, and can't be optional. Any type that passes these rules can be a
  key, such as an integer, an enum, a `type`, `uuid`, or a fixed-size
  struct. The key resolves where the kind is declared, so it can't name a
  type nested in the kind.
- The body takes what a struct's body takes, nested declarations and
  consts included. It is the object's state, the unit a hub stores, locks
  and caches. An object is one kind: a guild's bank is a field of `Guild`.
- A hub kind goes only at the top level of a namespace. One nested in a
  struct, a message or another kind is an error.
- Each kind has a `u16` id, dense from `1` over every kind, ordered by
  fully qualified name. See [Hub kind ids](wire-format.md#hub-kind-ids).

A hub kind is not a type. It is state with a key, as a component is, and
messages name it only as an endpoint, `hub<Guild>`. Naming it where a type
goes, in a field, a collection, an `option`, a union variant, a `type`
target or a key, is an error: "a hub kind is not a type; declare a struct
for what you send". What a client sees of an object is a struct of its own,
which the hub's code fills:

```
struct GuildInfo {
    name: string[max_length=24],
    level: u8,
}

message(hub<Guild> -> client) GuildChanged {
    info: GuildInfo,
}
```

So a field added to a kind never reaches clients by accident, and a kind's
body counts only toward the `internal`
[fingerprint](wire-format.md#fingerprint). A body is still encoded as a
struct with the same fields, for a hub's storage and its pushes to sims.

The Rust backend emits a hub kind as it does a struct, as an owned type and
a view, only for the `hub` role, which stores objects, and the `sim` role,
which caches them. Each implements `HubKind` in the root, with its key's
type, its id, and its qualified name. A role that sends to `hub<Guild>`
without the kind still gets the key's type, `GuildId`.

`hub` is a keyword only where a declaration starts, as `type` and `union`
are, so `hub: u8` is still a valid field.

## Components

Every ECS component that leaves its shard is declared in the schema: the
ones replicated to edges, clients and neighbor shards, and the ones that
travel with their entity on
[migration](../architectures/topology.md#entity-migration). Components that
never leave their shard are written by hand in Rust.

```
namespace demo.character

type FactionId = u32

audience Faction: FactionId

component Health {
    hp: u32,
    max_hp: u32,
}

@replicate(patch)
@visibility(owner)
component Stats {
    strength: u16,
    agility: u16,
}

@replicate(ops(InventoryOp))
@visibility(owner)
component Inventory {
    items: vec<Item>[max_length=64],
}

@visibility(none)
component Cooldowns {
    slots: array<u32, 8>,
}

tag Dead

@visibility(Faction)
tag Disguised
```

| Kind | Example | Declared in | Id on the wire |
|---|---|---|---|
| Seen by clients | `Health`, a transform | schema, `component` or `tag` | static, from the compiler |
| Crosses processes, seen by no client | `Cooldowns` | schema, `@visibility(none)` | static, from the compiler |
| Never leaves its shard, rebuilt on arrival | an AI cache, a physics body | Rust, `#[derive(Component)]` | none |

- `component`, `tag` and `audience` go only at the top level of a
  namespace. One nested in a struct, a message, a hub kind or a component
  is an error.
- A component's body takes what a struct's body takes, nested declarations
  and consts included, and its fields are laid out and checked as a
  struct's are.
- Each is a keyword only where a declaration starts, as `hub` is, so
  `tag: u8` is still a valid field.

> **Not generated yet.** `ujuc` checks components, tags and audiences, and
> gives them ids and fingerprint coverage, but the Rust backend and the
> [IR JSON](ir.md) leave them out for now.

### Tags

A component that carries no data is a `tag`. It has no body, so no value
and no patch: only its addition and removal go on the wire.

```
tag Dead
```

A component with no fields is an error that suggests a tag, "a component
with no fields is a tag; write `tag Dead`", so a tag has one spelling. That
holds for a body that declares only nested types or consts too. A tag
takes `@visibility` and `@halo`, but not `@replicate`: "a tag has no value
to replicate".

### Replication modes

`@replicate` says what a change to a component sends. Every component also
has a full encoding, which an entity's arrival, snapshots and resyncs send,
so the mode only decides what goes in a delta:

| Mode | A change sends | Fits |
|---|---|---|
| `replace`, the default | The whole value | Small components whose fields change together, such as a transform or health |
| `patch` | The top-level fields that changed | Structs with many fields, of which few change at once |
| `ops(Union)` | Operations of the named [union](#unions) | Large collections, or anything with its own semantics, such as an inventory |

- A `patch` is shallow: a nested struct or a collection that changes is sent
  whole.
- `ops` names a union, the component's op type. Naming anything else is an
  error, "`X` is not a union; the op type of `ops` must be a union". The
  user writes how each op applies, in every language that receives the
  component.
- The default is explicit, not derived from size, so adding a field never
  changes how a component travels.

### Visibility

`@visibility` says which clients receive a component or tag:

- `public`, the default: every client whose view covers the entity's cell,
  in the cell's [shared frame](../architectures/topology.md#shared-cell-frames).
- `owner`: only the client controlling the entity, outside cell frames.
- `none`: no client. Edges never receive it, and client code leaves it out.
- An [audience](#audiences), by path: the clients nearby whose observer
  holds the entity's key for that audience.

Any other path is an error, "`X` is not an audience; expected `public`,
`owner`, `none` or an audience".

### Halo

`@halo(true)` or `@halo(false)` says whether the neighbor sim shards that
[replicate](../architectures/topology.md#replicas-between-shards) a cell
receive the component. It defaults to `true`, except with
`@visibility(none)`, where it defaults to `false`. A missing `halo` would
only fail once two cells land on different shards, which moves with load,
so the default is the safe one. `@halo` takes its value: a bare `@halo` is
an error.

Migration takes no attribute: every component declared in the schema
travels with its entity.

### Audiences

An audience splits who sees a component nearby by a key the user declares:

```
type FactionId = u32

audience Faction: FactionId
audience Stealth: u8

@visibility(Faction)
component FactionChatter { line: u16 }

@visibility(Stealth)
tag Hidden
```

- An entity holds zero or one key per audience, and an observer, the
  client's character, holds a set of keys. A component reaches the
  observer when its set holds the entity's key.
- The key goes after a `:`, as a `const`'s type does, since an audience has
  no body. It follows the rules of a [hub key](#hub-kinds): fixed-size, no
  float, and not optional.
- A component or tag names one audience at most. Data two audiences see is
  split into two components.

### Attribute paths

The paths in `@replicate(ops(..))` and `@visibility(..)` resolve where the
component or tag is declared, at the top of its namespace, as a hub key
does, so `use` declarations apply. A union nested in the component itself
is not in scope there. A path that resolves to nothing is reported as any
unknown name is, so `@visibility(team)` gives "cannot find `team` in this
scope".

### Not types

Components, tags and audiences are state, not types, as hub kinds are.
Naming one where a type goes, in a field, a collection, an `option`, a
union variant, a `type` target or a key, is an error:

- "a component is not a type; declare a struct for what you send"
- "a tag is not a type; it has no value"
- "an audience is not a type; name its key's type instead"

### Ids

Components and tags share one `u16` id space, and audiences have one of
their own. See [Component ids](wire-format.md#component-ids) and
[Audience ids](wire-format.md#audience-ids). Components, tags and audiences
are part of the [fingerprints](wire-format.md#fingerprint).

The wire uses only these static ids. `uju::ecs` keeps assigning its own ids
at startup to index storage, since one `World` holds generated and
hand-written components alike. Those ids come from sorting the names of the
components a binary links, so they differ between binaries, such as a sim
and an edge, and never go on the wire.

### Open questions

- **Extra derives**: how a user adds derives or attributes to a generated
  type, by a schema attribute or a `ujuc` option.
