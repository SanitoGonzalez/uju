# IR JSON

`ujuc protocol ir` emits the compiled schema as JSON, together with the wire
layout of every declaration. It is meant for generators written outside
`ujuc`, such as engine-specific bindings for Unreal, Unity, or Godot. They read
this output instead of parsing `.uju` files, and copy layouts from it instead
of deriving them, so their code agrees with every built-in backend byte for
byte. [Wire Format](wire-format.md) explains what each layout value means.
[Foreign types](schema.md#foreign-types) mapped in `ujuc.toml` are not part
of the protocol, so the JSON leaves them out.

```sh
ujuc protocol ir -o gen/ir schemas/
```

## Files

| Path | Holds |
|---|---|
| `schema.json` | What spans namespaces: the fingerprints, the namespaces, and the id orders of messages, hub kinds, components and audiences |
| `namespaces/<name>.json` | One namespace's declarations, such as `namespaces/game.core.json` |

A namespace's file changes only when the namespace itself changes. Namespace
files sit in their own directory, so a namespace named `schema` cannot
collide with the root.

All numbers are JSON integers, except float constraints and float consts,
which keep their fraction (`1.0`). Fingerprints, `u64` values, and
`duration` and `timestamp` consts can exceed 2^53, so read them as 64-bit
integers, not as doubles.

## `schema.json`

```json
{
  "fingerprints": { "client": 5065904886572146457, "internal": 18401690520226133482 },
  "namespaces": ["demo.guild", "demo.world"],
  "links": [
    { "from": "client", "to": "sim", "messages": ["demo.guild.Ping"] },
    { "from": "client", "to": "hub", "messages": ["demo.guild.Rename"] },
    { "from": "sim", "to": "client", "messages": ["demo.guild.Ping"] },
    { "from": "hub", "to": "client", "messages": ["demo.guild.Renamed"] }
  ],
  "hubs": ["demo.guild.Guild"],
  "components": [
    "demo.world.Dead",
    "demo.world.Hidden",
    "demo.world.Inventory",
    "demo.world.Stats",
    "demo.world.Brain"
  ],
  "client_components": 4,
  "audiences": ["demo.world.Faction"]
}
```

- `fingerprints`: the `u64` `client` and `internal`
  [fingerprints](wire-format.md#fingerprint).
- `links`: each role pair some message travels, sorted by `from` then
  `to` in the order `client`, `entry`, `edge`, `sim`, `hub`. `messages`
  holds the pair's qualified message names in
  [id](wire-format.md#message-ids) order. The message at index `i` has id
  `i + 1`. Hub kinds share their role's pairs, as `client -> hub`.
- `hubs`: qualified hub kind names in [id](wire-format.md#hub-kind-ids)
  order. The kind at index `i` has id `i + 1`.
- `components`: qualified component and tag names in
  [id](wire-format.md#component-ids) order. The one at index `i` has id
  `i + 1`. Those clients see come first: `client_components` counts them,
  so they take ids `1` to `client_components`.
- `audiences`: qualified audience names in
  [id](wire-format.md#audience-ids) order. The audience at index `i` has
  id `i + 1`.

## Namespace files

```json
{
  "namespace": "foo.bar",
  "defs": [ ... ],
  "consts": [ ... ]
}
```

`defs` lists the namespace's declarations, nested ones included. Each has:

- `name`: its own name, as in `Result`
- `path`: its qualified name, as in `foo.bar.MyResponse.Result`, which is how
  other declarations refer to it
- `parent`: the `path` of the declaration it is nested in, or `null`
- `client`: whether code for clients holds it. See
  [Client declarations](#client-declarations)
- `kind`: `enum`, `struct`, `message`, `union`, `type`, `hub`, `audience`,
  `component`, or `tag`

### Enums

```json
{ "kind": "enum", "repr": "u8", "variants": [{ "name": "Red", "value": 0 }] }
```

An enum is stored on the wire as its `repr`.

### Structs and messages

A struct or message has a `layout` and `fields`. A message also has its
`links`:

```json
{
  "kind": "message",
  "links": [
    { "from": "client", "to": "hub", "id": 3, "from_kinds": null, "to_kind": "demo.guild.Guild" },
    { "from": "hub", "to": "client", "id": 2, "from_kinds": ["demo.guild.Guild"], "to_kind": null }
  ],
  "layout": {
    "size": { "kind": "var", "max": null },
    "bitmap": 1,
    "boundaries": 3,
    "boundary": 2,
    "head": 5,
    "vars": 2
  },
  "fields": [
    {
      "name": "x",
      "type": { ... },
      "optional": false,
      "slot": { "kind": "at", "offset": 1 }
    }
  ]
}
```

`links` lists the role pairs the message travels, sorted as in
`schema.json`. Each has:

- `from` and `to`: the roles, with any hub kind as `hub`.
- `id`: the message's [id](wire-format.md#message-ids) among the messages
  of that pair.
- `from_kinds`: the qualified names of the hub kinds that may send it,
  sorted, when `from` is `hub` and the message names them. `null` when any
  kind may send it, or when `from` is not `hub`.
- `to_kind`: the qualified name of the hub kind it goes to when `to` is
  `hub`, otherwise `null`.

`layout` describes the [object](wire-format.md#objects):

| Key | Meaning |
|---|---|
| `size` | The object's [size](#sizes) |
| `bitmap` | Bytes of the bitmap opening the head |
| `boundaries` | Offset of the first boundary, right after the fixed fields |
| `boundary` | Width of each boundary, in bytes |
| `head` | Bytes of the head: the bitmap, the fixed fields, and `vars - 1` boundaries |
| `vars` | Slots in the var section |

`fields` are in declaration order, which is the layout order. `slot` says
where each field sits. Bits count from the least significant bit of the
bitmap's first byte.

| `kind` | Other keys | Field |
|---|---|---|
| `bit` | `bit` | A `bool` or a `type` over one, stored as this bit |
| `at` | `offset` | A fixed-size value, `offset` bytes into the head |
| `optional_bit` | `presence`, `value` | An `option<bool>`, or an optional `type` over `bool`: its presence bit, then its value bit |
| `optional` | `presence` | An optional fixed-size value. Present values are packed back to back after the head, in declaration order |
| `var` | `slot`, `presence` | A variable-size value in this var slot. `presence` is its bit when optional, otherwise `null`. An absent value's slot is empty |

### Hub kinds

```json
{
  "kind": "hub",
  "id": 1,
  "key": { "kind": "named", "path": "demo.guild.GuildId", "constraints": {}, "wire": { ... } },
  "layout": { ... },
  "fields": [ ... ]
}
```

A [hub kind](schema.md#hub-kinds) has its `id`, from 1 as in `hubs` in
`schema.json`, and its `key`, a [type](#types) as a field's is. Its
`layout` and `fields` are exactly a struct's. Its `parent` is always
`null`, since kinds are declared only at the top level.

### Unions

```json
{
  "kind": "union",
  "size": { "kind": "var", "max": 6 },
  "tag": 1,
  "variants": [
    { "name": "Add", "tag": 1, "kind": "fields", "layout": { ... }, "fields": [ ... ] },
    { "name": "Remove", "tag": 2, "kind": "type", "type": { "kind": "u8", ... } },
    { "name": "Clear", "tag": 3, "kind": "unit" }
  ]
}
```

A [union](wire-format.md#unions) has a `size`, which is always `var`, a
`tag` giving the width of its tag in bytes (`1` or `2`), and its `variants`
in declaration order. Each variant has its `name`, its own `tag` value on
the wire, and a `kind`:

| `kind` | Other keys | Data |
|---|---|---|
| `unit` | | None |
| `type` | `type` | A single value of this [type](#types) |
| `fields` | `layout`, `fields` | An object, described as for [structs](#structs-and-messages) |

### Type declarations

```json
{ "kind": "type", "target": { "kind": "u8", "constraints": { "max": 100 }, "wire": { ... } } }
```

A `type` is encoded as its `target`, which is a scalar, or a `named` enum or
`type`.

### Audiences

```json
{
  "kind": "audience",
  "id": 1,
  "key": { "kind": "named", "path": "demo.world.FactionId", "constraints": {}, "wire": { ... } }
}
```

An [audience](schema.md#audiences) has its `id`, from 1 as in `audiences`
in `schema.json`, and its `key`, a [type](#types) as a field's is.

### Components

```json
{
  "kind": "component",
  "id": 4,
  "visibility": "public",
  "audience": null,
  "halo": true,
  "replicate": "patch",
  "ops": null,
  "layout": { ... },
  "fields": [ ... ],
  "patch": {
    "layout": {
      "size": { "kind": "var", "max": 33 },
      "bitmap": 1,
      "boundaries": 1,
      "boundary": 1,
      "head": 2,
      "vars": 2
    },
    "fields": [
      { "name": "alive", "dirty": 0, "some": null, "value": { "kind": "bit", "bit": 5 } },
      { "name": "hp", "dirty": 1, "some": null, "value": { "kind": "packed" } },
      { "name": "level", "dirty": 2, "some": 6, "value": { "kind": "packed" } },
      { "name": "title", "dirty": 3, "some": 7, "value": { "kind": "var", "slot": 0 } },
      { "name": "name", "dirty": 4, "some": null, "value": { "kind": "var", "slot": 1 } }
    ]
  }
}
```

A [component](schema.md#components) has:

- `id`: its [id](wire-format.md#component-ids), shared with tags, as in
  `components` in `schema.json`.
- `visibility`: which clients receive it: `public`, `owner`, `none`, or
  `audience`.
- `audience`: the qualified name of the audience when `visibility` is
  `audience`, otherwise `null`.
- `halo`: whether neighbor sim shards replicating its cell receive it,
  defaults applied.
- `replicate`: what its delta sends: `replace`, `patch`, or `ops`.
- `ops`: the qualified name of its op union when `replicate` is `ops`,
  otherwise `null`.
- `layout` and `fields`: its full encoding, exactly a struct's.
- `patch`: its [patch](wire-format.md#patches) when `replicate` is
  `patch`, otherwise `null`.

A patch's `layout` has the keys of an object's. `fields` lists where each
field sits in the patch, in declaration order, by `name`, as in the
component's `fields`:

- `dirty`: its dirty bit, set when it is sent.
- `some`: for an `option` field, its bit set when it is sent as `Some`,
  otherwise `null`.
- `value`: where its value sits when sent:

| `kind` | Other keys | Value |
|---|---|---|
| `bit` | `bit` | A `bool` or a `type` over one, stored as this bit |
| `packed` | | A fixed-size value. Values are packed back to back after the head, in declaration order |
| `var` | `slot` | A variable-size value in this var slot, empty when there is no value |

### Tags

```json
{ "kind": "tag", "id": 2, "visibility": "audience", "audience": "demo.world.Faction", "halo": true }
```

A [tag](schema.md#tags) has its `id`, `visibility`, `audience`, and `halo`,
as a component does. It has no value, so no layout.

### Client declarations

`client` is `true` for what the client
[fingerprint](wire-format.md#fingerprint) lists, which is what code for
clients holds: every message with a client end, every component and tag
whose `visibility` is not `none`, every audience, and every declaration they
reach. Every other declaration is `false`, such as a `visibility=none`
component and the types only it reaches, so clients never learn their
shapes.

A `true` declaration can be nested in a `false` one, such as a struct in a
`visibility=none` component that a public component names, or any const,
since every const is `true`. Its `path` still names its parent, so code for
clients places it under that path and leaves the parent's body out. Hiding a
sensitive name is the schema author's job.

A hub kind that a client message names is `true`, but clients know only its
`id`, `key`, and name. Its `fields` and `layout` are servers' only, and the
types only they reach are `false`.

### Types

A type is a tree:

```json
{
  "kind": "vec",
  "item": {
    "kind": "string",
    "constraints": { "max_length": 16 },
    "wire": { "size": { "kind": "var", "max": 16 } }
  },
  "constraints": { "max_length": 14 },
  "wire": {
    "size": { "kind": "var", "max": 238 },
    "sequence": { "kind": "prefixed", "count": 1, "prefix": 1 }
  }
}
```

`kind` is a scalar (`i8` to `i64`, `u8` to `u64`, `f32`, `f64`, `bool`,
`timestamp`, `duration`, `entity`, `uentity`, `uuid`, `string`, `bytes`) or
one of these:

| `kind` | Other keys |
|---|---|
| `array` | `item`, `length` |
| `vec`, `set` | `item` |
| `map` | `key`, `value` |
| `named` | `path`: the declaration's qualified name |

`constraints` holds only the constraints that are set: `min`, `max`,
`min_length`, `max_length`, and `infinite` and `nan` (only when `true`). It is
`{}` when no constraint is set. Floats are finite and not NaN unless the
constraints say otherwise.

`wire` says how values of the type are encoded:

- `size`: see [Sizes](#sizes). A `bool` reports 1 byte, which is its size
  inside an array or collection. Inside an object, its `slot` puts it in the
  bitmap instead.
- `int`, on integers: `{ "kind": "native", "width": 4 }`, stored as
  declared, or `{ "kind": "biased", "min": -100, "width": 1 }`, stored as
  `value - min` unsigned. See [Integers](wire-format.md#integers). A
  `named` type has none, even when it names a `type` over an integer: read
  it from the declaration's `target`.
- `sequence`, on `vec`, `set`, and `map`: how the
  [items](wire-format.md#collections) are laid out. A map's items are its
  entries, key then value. Widths are in bytes.

| `kind` | Other keys | Items |
|---|---|---|
| `packed` | `item` | `item` bytes each, back to back. The count is the length divided by `item` |
| `counted` | `count` | Zero bytes each, so only their count is sent, `count` bytes wide |
| `prefixed` | `count`, `prefix` | Of varying size: a count, then each item behind a `prefix`-byte length, except the last, which runs to the end |

### Sizes

A size is `{ "kind": "fixed", "bytes": 13 }`, or `{ "kind": "var", "max": 238 }`
for one that varies. `max` is `null` when only the 65535-byte message limit
bounds it.

### Consts

`consts` lists the namespace's consts, nested ones included, with `name`,
`path`, `parent`, and `client` as for declarations:

```json
{
  "name": "MaxAddresses",
  "path": "foo.bar.MyRequest.MaxAddresses",
  "parent": "foo.bar.MyRequest",
  "client": true,
  "type": "u8",
  "value": 14
}
```

`client` is always `true`, since both fingerprints list every const.

`type` is an integer type, `f32`, `f64`, `bool`, `string`, `duration`, or
`timestamp`, and `value` is the matching JSON value. A float keeps the value
written in source, so an `f32` const must be rounded to `f32` when read. A
`duration` is an integer of microseconds, and a `timestamp` an integer of
microseconds since the Unix epoch, so `30s` is `30000000` and
`2026-11-07T12:00:00Z` is `1794052800000000`.
