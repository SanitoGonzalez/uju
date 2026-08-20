# Wire

The schema is fixed: peers exchange a schema hash as the version, and there is
no cross-version compatibility. The encoded size of a value is computable
before writing, and reads are zero-copy — decoders should allocate receive
buffers at the message alignment (at most 8).

- little-endian; `bool` is 0 or 1; padding bytes are zero
- `string` is UTF-8; neither `string` nor `bytes` is terminated
- counts and byte lengths are u16 (at most 65535 items/bytes per container)

## Types

| type              | size (bytes)     | align     |
|-------------------|------------------|-----------|
| `i8`/`u8`/`bool`  | 1                | 1         |
| `i16`/`u16`       | 2                | 2         |
| `i32`/`u32`/`f32` | 4                | 4         |
| `i64`/`u64`/`f64` | 8                | 8         |
| `timestamp`       | 8                | 8         |
| `interval`        | 8                | 8         |
| `entity`          | 8                | 8         |
| `uentity`         | 12               | 4         |
| `uuid`            | 16               | 1         |
| enum              | repr size        | repr size |
| struct            | fields + padding | max field align |
| `array<T, N>`     | N × size(T)      | align(T)  |
| `string`/`bytes`  | variable         | 1         |
| `vec`/`set`/`map` | variable         | element/entry align |
| union             | variable         | max member align |

Scalars, enums, structs, and arrays are fixed-size; `string`, `bytes`, `vec`,
`set`, `map`, and unions are variable-size.

- struct fields must be fixed-size and non-optional: structs are memcpy-able
  PODs, and `vec<Struct>` payloads read as flat slices
- array items, set items, and map keys must be fixed-size
- messages are standalone request/response types and are never a field type

## Message layout

```
┌──────────────────────────────┐
│ fixed fields                 │
│ presence bits                │
│ u16 slots (counts/tags)      │
├──────────────────────────────┤
│ payload #1 (padding before)  │
│ payload #2                   │
│ ...                          │
└──────────────────────────────┘
```

- fixed fields are sorted by descending alignment (declaration order breaks
  ties) and packed at their natural alignment; an optional fixed-size field
  still reserves its inline space
- presence bits: one per optional field in declaration order, LSB-first,
  padded to a byte boundary
- slots: one u16 per variable-size field in declaration order, 2-aligned —
  an item count (`vec`/`set`/`map`), a byte length (`string`/`bytes`), or a
  member tag (union)
- payloads follow in declaration order, each preceded by padding up to its
  alignment

Structs pack the same way as the fixed-field region, with their size padded to
a multiple of their alignment.

encoded size = fixed section size, then per variable-size field in
declaration order: padding to the payload alignment + payload size
(recursively for nested blocks).

## Payloads

- `vec<T>`/`set<T>` with fixed-size `T`: count × size(T)
- `map<K, V>` with fixed-size `V`: count entries, key and value packed by the
  struct rule (higher alignment first, key first on ties)
- `string`/`bytes`: the raw bytes
- a variable-size element (vec/set item, map value, union member) is a
  **block**: `[u16 count/length/tag][padding][payload]`, aligned to
  max(2, its payload alignment); blocks are consecutive, so iteration is
  sequential rather than indexed
- `map` with variable-size `V`: count entries of `[key][padding][value
  block]`, aligned to max(align(K), block align of V)
- union: the u16 tag (slot or block head) is the member's declaration index;
  the payload is padded to the union's payload alignment (the max over all
  members) and holds the member's encoding — a fixed-size member's bytes, or
  a variable-size member's block
- `set` items and `map` entries are sorted ascending by item/key: numeric for
  integers, total order (`total_cmp`) for floats, field-wise in declaration
  order for structs
