# Schema

> **Status.** Only naming, consts, and the planned component declarations
> are written up here. Until the rest of the language is,
> `crates/uju-protoc/tests/data/example1.uju` shows every declaration kind,
> type and constraint.

## Naming

Names are ASCII: a letter or `_`, then letters, digits and `_`. Each kind of
name has a convention:

| Name | Convention | Example |
|---|---|---|
| Namespace segment | `snake_case` | `namespace demo.character;` |
| Enum, struct, message | `PascalCase` | `message[out] SpawnPlayer` |
| Enum variant | `PascalCase` | `Blue` |
| Const | `PascalCase` | `const MaxHp: u32 = 100;` |
| Field | `snake_case` | `max_hp: u32` |

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
namespace demo.chat;

const MaxNameLength: u16 = 16;
const TickRate: f32 = 30.0;
const PvpEnabled: bool = true;
const Greeting: string = "Welcome!\n";

message[in] SetName {
    const MinLength: u8 = 2;
    name: string[max_length=16],
}
```

- The type is an integer (`i8` to `i64`, `u8` to `u64`), a float (`f32`,
  `f64`), `bool`, or `string`, without constraints.
- An integer must fit its type. A float takes an integer or a float literal
  (`2` or `2.0`) and must be finite in its type. NaN and infinities can't be
  written.
- A string takes the escapes `\"`, `\\`, `\n`, `\r`, `\t`, `\0`, and
  `\u{1F600}` (one to six hex digits), and can hold any UTF-8. Its length is
  not capped, since it never goes on the wire.
- `true` and `false` are only literals where a value is expected, so they
  stay usable as names.

A const goes at the top of a namespace or in a struct or message, like a
nested type. Consts and types share each scope, so a const can't have the
name of a type declared beside it, and a type path that reaches a const is
an error. Consts never go on the wire, but they count toward the
[fingerprint](wire-format.md#fingerprint).

The Rust backend emits `pub const MAX_NAME_LENGTH: u16 = 16;` in the
namespace's module, and a nested const in its parent's module, as
`set_name::MIN_LENGTH`. A string is a `&str`.

> **TODO.** Constraints and array lengths can't refer to consts yet, as in
> `string[max_length=MaxNameLength]` or `array<u32, SlotCount>`.

## Components

> **TODO.** Nothing below is implemented yet.

Every ECS component that leaves its process is declared in the schema, and
the compiler generates it. That covers the components replicated to edges
and clients, and also the ones sent to another shard on
[migration](../architectures/topology.md#entity-migration) or written to
storage. Components that never leave their shard are written by hand in Rust.

```
namespace demo.character;

component Health {
    hp: u32,
    max_hp: u32,
}

component[visibility=owner] Inventory {
    items: vec<Item>[max_length=64],
}

component[visibility=server] Cooldowns {
    slots: array<u32, 8>,
}
```

| Kind | Example | Declared in | Id on the wire |
|---|---|---|---|
| Seen by clients | `Health`, a transform | schema, `component` | static, from the compiler |
| Crosses processes, seen by no client | `Cooldowns` | schema, `component[visibility=server]` | static, from the compiler |
| Never leaves its shard, rebuilt on arrival | an AI cache, a physics body | Rust, `#[derive(Component)]` | none |

### Visibility

`visibility` says which clients receive a component, and so how edges
deliver it (see [Shared cell frames](../architectures/topology.md#shared-cell-frames)):

- `public`, the default: every client whose view covers the cell, in the
  cell's shared frame.
- `team`: the clients of one visibility class, in a split of the cell's frame.
- `owner`: only the client controlling the entity, outside cell frames.
- `server`: no client. Edges never receive it.

### Ids

Each component gets a `u16` id, in its own dense space from `1`, ordered by
fully qualified name, like [message ids](wire-format.md#message-ids).
Components are part of the [fingerprint](wire-format.md#fingerprint).

The wire uses only these static ids. `uju::ecs` keeps assigning its own ids
at startup to index storage, since one `World` holds generated and
hand-written components alike. Those ids come from sorting the names of the
components a binary links, so they differ between binaries, such as a sim
and an edge, and never go on the wire.

### What the compiler generates

- The owned Rust type and its `Component` registration, which carries the
  static id.
- A view, as for any struct.
- A patch type with `diff` and `apply`, as the planned `struct[patch]`
  attribute would generate (see [Wire Format](wire-format.md#todo)), so
  replication sends only the changed fields.

The generated code goes into the user's shared crate rather than a crate of
its own, so the user can add inherent `impl` blocks to the generated types.

### Open questions

- **Math types**: which schema types map to the Rust math types systems
  compute with, such as `parry3d::glamx::Vec3`, instead of packed wire
  structs.
- **Extra derives**: how a user adds derives or attributes to a generated
  type, by a schema attribute or a `ujuc` option.
- **Built-in components**: `uju::ecs` ships `Transform3` and `Velocity3`
  written by hand. Replicating them takes a schema of uju's own, which users
  compile with theirs.
- **Halo replicas**: whether `server` components go to the neighbor sim
  shards that replicate a cell, or only `public` ones do.
