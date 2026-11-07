# Wire Format

How a uju message is encoded. The compiler computes every layout described
here (`crates/uju-protoc/src/ir/wire.rs`); backends read it from there instead
of deriving it, so every target agrees byte for byte. Generators outside
`ujuc` read it from [`ujuc protocol ir`](ir.md).

## Principles

- **The schema fixes the layout completely.** There are no field tags,
  vtables, or schema evolution. Both peers must be built from the same schema,
  which they confirm by comparing [fingerprints](#fingerprint). Checking them is
  the application's job.
- **Zero-copy views.** A decoder validates a message once, then reads fields
  straight out of the buffer. Fixed-size fields sit at constant offsets, and
  variable-size fields are found through a small table of boundaries.
- **Packed, unaligned, little-endian.** Nothing is padded. Messages can start
  at any byte of a receive buffer, so readers must use unaligned loads:
  align-1 wrapper types in Rust, `BinaryPrimitives`/`MemoryMarshal.Read` in
  C#, and `memcpy` in C++.
- **Constraints choose widths.** Since nothing migrates, the encoding may
  depend on constraints. Integer ranges narrow integers, and `max_length`
  narrows counts, lengths, and offsets.
- **A type encodes the same way wherever it appears.** Its constraints are
  part of the type. Offsets are relative to the start of their own object, so
  an encoded value can be copied verbatim into another message.
- **Sans I/O.** The protocol defines how a message body is encoded. Framing
  (message id, length, batching) belongs to the application. See
  [Envelope](#envelope).

## Limits

A message is at most **65535 bytes**. Every length, count, and offset
therefore fits in a `u16`, and `min_length`/`max_length` may not exceed 65535.
An encoder must refuse a value that would exceed the limit.

String and `bytes` lengths count **bytes**: UTF-8 bytes for strings, not
characters. Collection lengths count **items**.

## Scalars

| Type | Bytes | Encoding |
|---|---|---|
| `u8` `u16` `u32` `u64` | 1, 2, 4, 8 | Unsigned, or narrowed (see [Integers](#integers)) |
| `i8` `i16` `i32` `i64` | 1, 2, 4, 8 | Two's complement, or narrowed |
| `f32` `f64` | 4, 8 | IEEE 754 |
| `bool` | 1 bit or 1 byte | A bit in an object's [bitmap](#objects); a byte (`0` or `1`) inside an `array` or collection |
| `timestamp` | 8 | `i64` milliseconds since the Unix epoch, UTC |
| `interval` | 8 | `i64` milliseconds |
| `entity` | 8 | Index `u32`, then generation `u32` (non-zero) |
| `uentity` | 12 | Serial `u64`, then node `u16`, then shard `u16` |
| `uuid` | 16 | RFC 9562 byte order, as in its text form (the one big-endian value) |
| enum | 1, 2, 4, 8 | The declared repr (`u32` by default) holding the variant's value |

### Integers

An integer's range is its `min` and `max` constraints, or the declared type's
limits where one is absent. When the narrowest of 1, 2, 4, or 8 bytes that
holds `max - min` is narrower than the declared type, the integer is stored
**biased**: `value - min` as an unsigned integer of that width. Otherwise it
is stored as declared.

| Type | Stored as |
|---|---|
| `i32[min=-100, max=100]` | `value + 100` in 1 byte |
| `i64[min=0, max=70000]` | `value` in 4 bytes |
| `u32[min=1000, max=1255]` | `value - 1000` in 1 byte |
| `u8[max=9]` | `value` in 1 byte (already as narrow as it gets) |
| `i32[min=0]` | `value` as an `i32` (no narrower) |

Floats and enums are never narrowed. Float `min`/`max` and the finite/non-NaN
rules are only checked.

### Types

A `type` is encoded exactly as its target, constraints included, wherever
it appears. `type Level = u8[max=100]` is a byte. `type Flag = bool` takes
a bit in an object's [bitmap](#objects), two bits as an `option<Flag>`
field, and a byte inside an `array` or collection, as a `bool` does.

## Fixed-size and variable-size types

A type is **fixed-size** when every value takes the same number of bytes:

- scalars, enums, and `type`s;
- `string` and `bytes` whose `min_length` equals their `max_length`
  (`bytes[min_length=32, max_length=32]` is always 32 bytes);
- `array<T, N>`, which is `N` values of `T` back to back. `T` must be
  fixed-size;
- structs whose fields are all fixed-size and not optional, except
  `option<bool>`, which takes two bits either way.

Everything else is **variable-size**: other strings and bytes, `vec`, `set`,
`map`, structs holding a variable-size or optional field, and unions. A
variable-size value is always handed its exact byte range by whatever contains
it (the message, a boundary, or a length prefix), so it never records its own
length.

## Objects

Structs and messages are **objects**, and both are laid out the same way:

```
object := head | optional section | var section
head   := bitmap | fixed fields | boundaries
```

**Bitmap.** One bit per `bool` field, per optional field's presence, and per
`option<bool>`'s value. Bits are assigned in declaration order, starting from
the least significant bit of the first byte. The bitmap takes `ceil(bits / 8)`
bytes, none when no field needs a bit, and unused bits are zero. A field takes
these bits:

| Field | Bits |
|---|---|
| `bool` | its value |
| `option<bool>` | presence, then value (zero when absent) |
| any other `option<T>` | presence |

**Fixed fields.** Every non-optional fixed-size field other than a `bool`, in
declaration order, packed at constant offsets right after the bitmap.

**Boundaries.** Variable-size fields take numbered **slots** in declaration
order, and optional variable-size fields take one too. The head ends with
`slots - 1` boundaries. Boundary `k` is the end of slot `k`, as an offset from
the start of the object. Slot `0` starts where the optional section ends, and
every later slot starts where the one before it ends. The last slot runs to
the end of the object, so it needs no boundary. An object with one slot has no
boundaries at all.

Boundaries are 1 byte wide when the object's largest possible encoding, with
1-byte boundaries, fits in 255 bytes. Otherwise they are 2 bytes wide.
Variable-size values without a `max_length` make an object unbounded, so its
boundaries take 2 bytes.

**Optional section.** Present optional fixed-size values, packed in
declaration order. An absent one takes no bytes. The offset of the `k`-th
optional value is the head size plus the sizes of the present ones before it.

**Var section.** The slots' bytes, in order. An absent optional
variable-size value leaves its slot empty: its boundary equals the previous
one.

An object with no slots and no optional fixed-size values is fixed-size. Its
size is its head.

An object nested in another is stored as a value of its type: a fixed-size
struct inline, at its offset among the fixed fields or items, and a
variable-size struct in a slot or behind a length prefix.

### Example

```
message[in] MyRequest {
    foo: i32[min=-100, max=100],
    name: string[min_length=2, max_length=12],
    addresses: vec<string[max_length=16]>[max_length=14],
}
```

The largest encoding is 252 bytes, so the boundary is 1 byte wide:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `foo`, biased by 100 |
| 1 | 1 | boundary: end of `name` (slot 0) |
| 2 | ... | `name` (slot 0), then `addresses` (slot 1) |

`MyRequest { foo: 7, name: "bob", addresses: ["a", "bc"] }` encodes to 10
bytes:

```
6B             foo: 7 + 100
05             name ends at 5
62 6F 62       "bob"
02             addresses: 2 items
01             item 0 is 1 byte
61             "a"
62 63          "bc", the last item, runs to the end
```

### Example: a patch

Optional fields make sparse updates cheap. An absent field costs only a bit.

```
struct StatsPatch {
    hp: option<u16>,       // presence bit 0
    mp: option<u16>,       // presence bit 1
    alive: option<bool>,   // presence bit 2, value bit 3
    strength: option<u8>,  // presence bit 4
}
```

`StatsPatch { hp: Some(500), alive: Some(true), .. }` encodes to 3 bytes:

```
0D             bitmap: hp, alive present; alive is true
F4 01          hp: 500
```

## Collections

Collections are read by **iteration only**: there is no indexed access into
`vec`, `set`, or `map`. A decoder that needs lookup should convert the
collection into an owned type. A collection is variable-size, so it lives in a
slot or behind a length prefix, and it always knows its exact byte range.

`set<T>` is laid out like `vec<T>`. `map<K, V>` is laid out like a `vec` of
entries, where each entry is the key followed by the value. Set items and map
keys must be fixed-size and may not hold floats. Neither ordering nor
uniqueness is enforced on the wire: checking for duplicates is the
application's job.

An item is laid out one of three ways, depending on its size:

**Fixed-size items** (packed). The items sit back to back, and their count is
the byte length divided by the item size. A decoder may expose them as a slice
of align-1 views.

```
item₀ item₁ ... itemₙ₋₁
```

**Items of no size** (counted), such as an empty struct. Only the count is
sent.

```
count
```

**Variable-size items** (prefixed). A count, then each item behind its byte
length, except the last item, which runs to the end. An empty collection is
just its count.

```
count | len₀ item₀ | len₁ item₁ | ... | itemₙ₋₁
```

Widths:

- A **count** is 1 byte when the collection's `max_length` is at most 255, and
  2 bytes otherwise.
- A **length prefix** is 1 byte when an item's largest encoding is at most 255
  bytes, and 2 bytes otherwise (including when it is unbounded).

Option elements are not allowed in collections. `bool` items take a byte each.

## Unions

A union is the tag of the variant it holds, then that variant's data:

```
union := tag | data
```

**Tag.** Variants are tagged from `1` in declaration order, so variant `i`,
counting from `0`, is tagged `i + 1`, and `0` is never a tag. The tag is a
`u8` when the union has at most 255 variants, and a little-endian `u16`
otherwise.

**Data.** The data runs from the tag to the end of the union's byte range:

| Variant | Data |
|---|---|
| `C` | Nothing |
| `B(T)` | A value of `T`. A fixed-size `T` takes exactly its size, and a `bool` takes a byte |
| `A { x: T, ... }` | An [object](#objects) laid out like a struct with these fields. Its offsets count from its own start, right after the tag |

A union is variable-size even when every variant is fixed-size, so like any
variable-size value it sits in a slot or behind a length prefix and is
handed its exact byte range. Its largest encoding is the tag plus the
largest variant's data, and it is unbounded when any variant is.

### Example: an inventory operation

```
union InventoryOp {
    Add { item: u32, slot: u8 },
    Remove(u8),
    Clear,
}
```

Three variants take a 1-byte tag, and the largest encoding is 6 bytes:

```
01 07 00 00 00 02    Add { item: 7, slot: 2 }
02 03                Remove(3)
03                   Clear
```

## Validation

Decoding validates the whole message once. After that, reading a field cannot
fail. A decoder must reject a message unless:

- every boundary lies between the start of the var section and the end of the
  object, and boundaries never decrease;
- packed collections have a byte length that is a multiple of the item size,
  and counted and prefixed collections consume their range exactly;
- unused bitmap bits, the value bit of an absent `option<bool>`, and the slots
  of absent optional values are empty or zero;
- `bool` bytes are `0` or `1`, enums hold a declared variant, and `entity`
  generations are non-zero;
- union tags name a declared variant, never `0` or past the last one, a
  unit variant's data is empty, and a fixed-size variant type's data is
  exactly its size;
- strings are valid UTF-8;
- every constraint holds: integer and float ranges (a biased integer must not
  exceed `max - min`), finite and non-NaN floats unless `infinite`/`nan` is
  allowed, and `min_length`/`max_length`.

Encoders check the same constraints, since a narrowed integer cannot even
represent a value outside its range.

## Message ids

Every message has a `u16` id in each direction it travels:

- **inbound** (the messages a server receives): `in` and `bi` messages;
- **outbound** (the messages a server sends): `out` and `bi` messages.

Each space is dense from `1`, ordered by fully qualified name
(`namespace.Message`); `0` is never an id, so a zeroed id reads as no message. A `bi` message has an id in both spaces, and the two may
differ.

## Fingerprint

The compiler emits a `u64` fingerprint of the schema. It is the 64-bit
FNV-1a hash of a canonical text listing every declaration, sorted by fully
qualified name, with every name included: enums with their repr and variant
values, structs and messages with their fields in declaration order, unions
with their variants and each variant's data in declaration order, `type`s
with their target, each type spelled as in source, qualified, and with its
constraints, and messages with their direction. A union is listed as
`union a.U { A { x: u8, }, B(u8), C, }`, and a `type` as
`type a.Level = u8[max=100];`. Consts follow, sorted the same way, with
their type and value. They never go on the wire, but peers must agree on
them too.

Renaming anything, reordering fields or variants, or changing a constraint
or a const changes the fingerprint. The order in which sources are compiled does not. FNV-1a is not
collision-resistant against an attacker, and it doesn't need to be: it only
catches peers built from different schemas.

## Envelope

The protocol doesn't frame messages. The application must send enough to
decode one, typically the message id and the body's byte length, and may
batch, compress, or fragment as it likes. Fixed-size messages need no length,
because the id determines it.

## TODO

- `length=N` constraint as shorthand for `min_length=N, max_length=N`.
- Float quantization: `f32[min=..., max=..., step=...]`, stored as a biased
  integer, possibly narrowed to any width from 1 to 8 bytes.
- `struct[patch]` attribute: generate a patch struct (every field optional)
  with `diff` and `apply`, for replicating ECS components.
- Component ids: the compiler assigns every component declared in the
  schema a static id, as it does for messages, and replication names
  components by it. The ids `uju::ecs::component` assigns at startup come
  from sorting the names of the components a binary links, so they are only
  valid inside that binary: a sim that links one component an edge does not
  shifts every id after it. See [Components](schema.md#components).
- Rust backend: mark the generated code `#[rustfmt::skip]`, so `cargo fmt`
  leaves it alone and regenerating it changes nothing. Today the output is
  not formatted as rustfmt would, and users have to exclude its directory
  with rustfmt's `ignore`, which only nightly rustfmt supports. An inner
  `#![rustfmt::skip]` is unstable too, so the attribute goes on each
  generated item and on the root's `mod` declarations.
- `f16`.
