# IR JSON

`ujuc protocol ir` emits the compiled schema as JSON, together with the wire
layout of every declaration. It is meant for generators written outside
`ujuc`, such as engine-specific bindings for Unreal, Unity, or Godot. They read
this output instead of parsing `.uju` files, and copy layouts from it instead
of deriving them, so their code agrees with every built-in backend byte for
byte. [Wire Format](wire-format.md) explains what each layout value means.

```sh
ujuc protocol ir -o gen/ir schemas/
```

## Files

| Path | Holds |
|---|---|
| `schema.json` | What spans namespaces: the fingerprint, the namespaces, and message id order |
| `namespaces/<name>.json` | One namespace's declarations, such as `namespaces/game.core.json` |

A namespace's file changes only when the namespace itself changes. Namespace
files sit in their own directory, so a namespace named `schema` cannot
collide with the root.

All numbers are JSON integers, except float constraints and float consts,
which keep their fraction (`1.0`). `fingerprint` and `u64` values can exceed
2^53, so read them as 64-bit integers, not as doubles.

## `schema.json`

```json
{
  "fingerprint": 17932262614053035048,
  "namespaces": ["foo.bar", "hello"],
  "inbound": ["foo.bar.MyRequest", "foo.bar.Ping"],
  "outbound": ["foo.bar.MyResponse", "foo.bar.Ping"]
}
```

- `fingerprint`: the `u64` [fingerprint](wire-format.md#fingerprint).
- `inbound` / `outbound`: qualified message names in [id](wire-format.md#message-ids)
  order. The message at index `i` has id `i + 1`.

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
- `kind`: `enum`, `struct`, `message`, `union`, or `type`

### Enums

```json
{ "kind": "enum", "repr": "u8", "variants": [{ "name": "Red", "value": 0 }] }
```

An enum is stored on the wire as its `repr`.

### Structs and messages

A struct or message has a `layout` and `fields`. A message also has a
`direction` (`in`, `out`, or `bi`) and `ids`, which give its id in each
direction it travels and `null` in the other:

```json
{
  "kind": "message",
  "direction": "bi",
  "ids": { "inbound": 3, "outbound": 2 },
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
`path`, and `parent` as for declarations:

```json
{
  "name": "MaxAddresses",
  "path": "foo.bar.MyRequest.MaxAddresses",
  "parent": "foo.bar.MyRequest",
  "type": "u8",
  "value": 14
}
```

`type` is an integer type, `f32`, `f64`, `bool`, or `string`, and `value` is
the matching JSON value. A float keeps the value written in source, so an
`f32` const must be rounded to `f32` when read.
