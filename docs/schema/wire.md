# uju wire format

Version 0. This document specifies how compiled schema types are encoded on
the wire.

## Principles

- **Fixed protocol.** No field tags, no vtables, no per-field versioning.
  Both peers must compile the exact same schema; a canonical hash of the
  compiled schema gates the connection. Every layout decision below is
  static and precomputed by the compiler.
- **Little-endian, packed.** All multi-byte values are little-endian. There
  is no padding or alignment anywhere; readers use unaligned loads.
- **16-bit sizes.** All counts, lengths, and offsets are `u16`. A single
  encoded record is therefore at most 65,535 bytes and a container holds at
  most 65,535 elements. Batching large payloads into multiple messages is
  the user's responsibility.
- **Canonical.** Equal values always encode to identical bytes: sets and
  maps are sorted, absent optional slots are zero-filled, and there are no
  encoder degrees of freedom. Encodings can be compared, hashed, and diffed
  as raw bytes.
- **Deterministic size.** The encoded size of a value is computable in one
  pass without writing, so the destination buffer can be allocated exactly
  once. Nothing in the format requires a growing buffer.

## Primitives

| type        | size (bytes) | encoding                                        |
|-------------|--------------|-------------------------------------------------|
| `i8`–`i64`  | 1/2/4/8      | two's complement                                 |
| `u8`–`u64`  | 1/2/4/8      | unsigned                                         |
| `f32`/`f64` | 4/8          | IEEE 754                                         |
| `bool`      | 1            | 0 or 1; other values are invalid                 |
| `timestamp` | 8            | i64 microseconds since Unix epoch, UTC           |
| `interval`  | 8            | i64 microseconds                                 |
| `entity`    | 8            | local entity (index, generation); opaque         |
| `uentity`   | 12           | universal entity (node, shard, index, generation); opaque |
| `string`    | variable     | `u16 len` + UTF-8 bytes, no terminator           |
| `bytes`     | variable     | `u16 len` + raw bytes                            |

An enum encodes as its repr (`u32` by default).

## Records

`struct`, `component`, and `message` share one layout, decided per type at
compile time:

```
[ presence bitmap ][ fixed part ][ var heap ]
```

- **Presence bitmap**: one bit per *optional fixed-size* field, in field
  declaration order, LSB-first within each byte. `⌈n/8⌉` bytes; omitted
  entirely when there are no optional fixed-size fields. Bit set = present.
- **Fixed part**: fields in declaration order. A fixed-size field is
  encoded inline. A variable-size field occupies a 2-byte slot holding a
  `u16` offset to its payload. Field offsets and the fixed part's total
  size are compile-time constants.
- **Var heap**: payloads of variable-size fields, in field order,
  immediately after the fixed part.

Every offset is relative to the start of the thing that owns the offset
table: a record's field slots are relative to the record start, a
container's offset table is relative to the container start. Each encoding
is therefore self-contained and composes without threading a base offset
through nested values. Since payloads always live at or after the fixed
part, a payload offset is never 0, and `0` in a field slot means "absent"
for optional variable-size fields.

Payloads are **strictly sequential**: the first present variable-size
payload starts exactly at the end of the fixed part, and each subsequent
one starts exactly where the previous ended, in field order. Offsets are
therefore fully derivable — they exist as an O(1) random-access index, and
validation rejects any encoding whose offsets deviate. This is what makes
the format canonical rather than merely well-formed.

Optional semantics:

- optional fixed-size field: presence bit in the bitmap; the value slot is
  always present and must be zero-filled when absent.
- optional variable-size field: offset slot `0` when absent; no bitmap bit.
- optional is only allowed on fields, not inside containers.

A `struct` must be fixed-size all the way down (this is checked at compile
time), so its encoding is exactly its fixed part and its size is constant.
`component` and `message` may be fixed- or variable-size. A record with no
fields encodes as zero bytes.

### memcpy fast path

When a generated Rust struct has no padding and its natural field layout
matches the wire layout, whole values (and columns of values) can be
encoded/decoded with `memcpy`. Codegen detects this per type with a
compile-time check; all other types use per-field encoding. Packed records
concatenate back-to-back with no inter-record padding, so a batch is
`u16 count` + records.

## Containers

Container payloads live in the enclosing record's var heap.

- `vec<T>`, `T` fixed-size: `u16 count` + `count` packed elements.
- `vec<T>`, `T` variable-size: `u16 count` + `u16 offsets[count]` +
  payloads.
- `set<T>`: same as `vec<T>`, with elements unique and sorted in canonical
  order. Elements must not be containers.
- `map<K, V>`: `u16 count` + `u16 values_start` + key column + value
  column. Each column is packed elements (fixed-size) or a `u16` offset
  table + payloads (variable-size). `values_start` locates the value
  column in O(1) even when keys are variable-size; validation checks it
  against the canonical position. Keys are unique and sorted in canonical
  order; the i-th value belongs to the i-th key. Keys must not be
  containers; values are unrestricted.

Splitting a map into a key column and a value column (rather than
interleaving pairs) keeps the keys cache-dense for the binary search that
lookup performs.

An empty container is `count = 0`; it is still present (its offset slot
points at the count).

### Canonical order

Used for sorting set elements and map keys, and by binary search at read
time. The order is **structural**, not a comparison of encoded bytes, so
it can be evaluated on in-memory values without encoding them first:

- integers, `bool`, enums (by repr), `timestamp`, `interval`: ascending
  numeric.
- `f32`/`f64`: IEEE 754 total order (Rust's `total_cmp`), so NaN is
  ordered deterministically rather than being incomparable.
- `string`/`bytes`: bytewise lexicographic; a proper prefix sorts first.
- `entity`/`uentity`: numeric, fieldwise over their components.
- records: fieldwise in declaration order, each field by its own canonical
  order. An absent optional field sorts before a present one.
- containers (as record fields): elementwise, then by length.

## Reading and writing

**Write** is two passes: `encoded_size(&value)` walks the value summing
fixed part + payload sizes, the caller allocates exactly that, and
`encode(&value, buf)` fills it, appending payloads and back-filling offset
slots with a single cursor.

**Read** is zero-copy: generated view types wrap `&[u8]` and decode fields
on access with unaligned loads. Container views iterate without heap
allocation; map/set views additionally support O(log n) lookup by binary
search over the sorted key column.

**Validation**: view accessors do no well-formedness checks — they assume
canonical input. Untrusted bytes (anything that crossed the network) must
pass `validate(&[u8])` once before views are constructed. Validation walks
the value recursively and checks: buffer bounds against the fixed part,
every offset exactly at its canonical sequential position, counts
consistent with available space, `bool` in {0, 1}, enum values against
known variants, UTF-8 well-formedness of strings, zero-filled absent
optional slots, zero-filled unused bitmap bits, and sortedness/uniqueness
of sets and map keys. It returns the total consumed length. Trusted
intra-process bytes may skip validation.

Accessors remain memory-safe on unvalidated input (they are bounds-checked
slice reads, so a malformed buffer panics rather than reading out of
bounds), but only validated input is guaranteed to be panic-free.

## Rust representation

The `uju::wire` module provides the runtime that generated Rust links
against. Schema types map as follows:

| schema        | owned Rust             | zero-copy view      |
|---------------|------------------------|---------------------|
| `i32`, `f64`… | `i32`, `f64`…          | same                |
| `timestamp`   | `uju::wire::Timestamp` | same                |
| `interval`    | `uju::wire::Interval`  | same                |
| `entity`      | `uju::wire::Entity`    | same                |
| `uentity`     | `uju::wire::UEntity`   | same                |
| `string`      | `String`               | `&'a str`           |
| `bytes`       | `Vec<u8>`              | `&'a [u8]`          |
| `vec<T>`      | `Vec<T>`               | `VecView<'a, T>`    |
| `set<T>`      | `uju::wire::Set<T>`    | `SetView<'a, T>`    |
| `map<K, V>`   | `uju::wire::Map<K, V>` | `MapView<'a, K, V>` |
| `enum E`      | `E`                    | same                |
| record `R`    | `R`                    | `RRef<'a>`          |
| optional `T?` | `Option<T>`            | `Option<view>`      |

`Set` and `Map` are sorted-vec wrappers, not `BTreeSet`/`BTreeMap`. They
mirror the wire layout exactly, enforce the canonical order as a
construction invariant (so `encode` never has to sort and stays
allocation-free), and avoid requiring `Ord` on user types — which records
containing `f32` cannot provide.

The core traits are `Wire` (encode side: `FIXED_SIZE`, `encoded_size`,
`encode`), `View<'a>` (read side: `read`, `validate`, `owned`), and
`Canonical` (the ordering above). Records also implement the marker traits
`Component`, `Message` (carrying `MESSAGE_ID`), and `Request` (carrying
`type Response`) according to their kind.

## Messages

Each `message` gets a sequential `u32` id in declaration order; the schema
hash guards against accidental renumbering. `message A -> B` records B as
the response type of A in the compiled schema. Framing — message id on the
wire, request/response correlation, batching — belongs to the transport
layer, not this format.
