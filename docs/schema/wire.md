# Wire

## Types

| type        | size (bytes) |
|-------------|--------------|
| `i8`–`i64`  | 1/2/4/8      |
| `u8`–`u64`  | 1/2/4/8      |
| `f32`/`f64` | 4/8          |
| `bool`      | 1            |
| `timestamp` | 8            |
| `interval`  | 8            |
| `entity`    | 8            |
| `uentity`   | 12           |
| `uuid`      | 16           |
| `array`     | fixed        |
| `string`    | variable     |
| `bytes`     | variable     |
| `vec`       | variable     |
| `map`       | variable     |
| `set`       | variable     |

## Layout

```
┌──────────────────────────────┐
│ fixed fields                 │
│ presence bits                │
│ lengths/counts of containers │
│ (paddings)                   │
├──────────────────────────────┤
│ vec #1 contents (+paddings)  │
│ vec #2 contents              │
│ map contents                 │
│ set contents                 │
└──────────────────────────────┘
```

- little-endian
- lengths/counts are encoded to 2 bytes (maximum 65535 items)

## Containers

- **set**: represented as sorted items
- **map**: represented as sorted entries
