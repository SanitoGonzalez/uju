# Audiences

How a `World` holds each entity's key for an [audience](../protocol/schema.md#audiences)
and each observer's set of keys, and how keys travel on the halo stream. A
component reaches an audience's clients through its
[`Visibility::Audience`](change-tracking.md#replicated-components), and the
sub-frames that carry it are in [Frames](../protocol/frames.md#sub-frames).

## The Audience trait

An audience is a marker type implementing `Audience`, one per
`audience Name: Key` declaration of the schema:

```rust
pub trait Audience: 'static {
    type Key: Copy + Eq + Hash + Debug + 'static;
    const WIRE_ID: u16;
    const KEY_SIZE: usize;

    fn id() -> Id;
    fn encode_key(key: &Self::Key, buf: &mut [u8]) -> Result<usize, EncodeError>;
    fn decode_key(buf: &[u8]) -> Result<Self::Key, DecodeError>;
}
```

| Item | Meaning |
|---|---|
| `Key` | The key type. Fixed-size, with no floats. `Debug` shows it in [`world.inspect`](entities.md#inspecting) |
| `WIRE_ID` | The [audience id](../protocol/wire-format.md#audience-ids) |
| `KEY_SIZE` | The size of every key's encoding |
| `id()` | The index of its table in a `World` |
| `encode_key` | Writes a key's `KEY_SIZE` bytes to the start of `buf` |
| `decode_key` | Decodes a key from exactly `KEY_SIZE` bytes, validating it |

The encoding lives on the audience rather than on the key, so a key can be
a primitive, such as `u8`, or a type of another crate.

- **Registered.** An audience registers in `AUDIENCES` as a component does
  in `COMPONENTS`, with a `Registration` whose `new_table` builds its table.
  `uju::init()` numbers audiences in order of their `name`, dense from `0`,
  and each `World` builds one table per audience.
- **Not a component.** Keys are outputs of gameplay state, which systems
  already query, so an audience has no query term.

```rust
use std::cell::UnsafeCell;
use uju::ecs::audience::{AUDIENCES, Audience, Id, Registration, new_table};
use uju::net::wire::{DecodeError, EncodeError};

pub struct Stealth;

#[uju::linkme::distributed_slice(AUDIENCES)]
#[linkme(crate = uju::linkme)]
static STEALTH: Registration = Registration {
    name: concat!(module_path!(), "::Stealth"),
    id: UnsafeCell::new(u16::MAX),
    new_table: new_table::<Stealth>,
};

impl Audience for Stealth {
    type Key = u8;
    const WIRE_ID: u16 = 2;
    const KEY_SIZE: usize = 1;

    fn id() -> Id {
        unsafe { *STEALTH.id.get() }
    }

    fn encode_key(key: &u8, buf: &mut [u8]) -> Result<usize, EncodeError> {
        buf[0] = *key;
        Ok(1)
    }

    fn decode_key(buf: &[u8]) -> Result<u8, DecodeError> {
        Ok(buf[0])
    }
}
```

`name` must be unique among audiences, since ids follow it.

## Keys

An entity holds zero or one key per audience.

| Method | Does |
|---|---|
| `world.set_audience_key::<A>(entity, Some(key))` | Sets the key, returning the previous one |
| `world.set_audience_key::<A>(entity, None)` | Clears the key, returning the previous one |
| `world.audience_key::<A>(entity)` | The key, or `None` |

```rust
world.set_audience_key::<Faction>(player, Some(FactionId::RED));
let tier = world.audience_key::<Stealth>(target).unwrap_or(0);
```

- **Only writer.** Nothing else writes an audience's table. Setting the key
  an entity already holds, or clearing a key it does not hold, does
  nothing. So does setting the key of a dead entity.
- **Replicas.** A replica's key comes from its owner, and reads as an owned
  entity's.
- **Despawn.** A despawn drops the entity's keys and observer sets.
- **Borrows.** Each audience's table is a borrow of its own, apart from the
  component tables, so a system can set keys inside a query's loop.
  Setting a key while a borrow of the same audience's table is alive
  panics.

## Observer sets

An observer, such as a client's character, holds a set of keys per
audience. It sees an entity's components of that audience when its set
holds the entity's key.

| Method | Sets |
|---|---|
| `world.set_observer_keys::<A>(observer, keys)` | The keys in `keys`, any `IntoIterator<Item = A::Key>`. An empty set sees no key |
| `world.observe_all::<A>(observer)` | Every key, such as for a GM |
| `world.observe_own_key::<A>(observer)` | The default: the observer's own key |

- **Default.** An observer's set is its own key unless one is stored, and
  storing nothing is what the default costs. The set follows the key: an
  observer whose key changes sees the new key's entities. An observer with
  no key sees none.
- **No-op.** Storing the set already stored does nothing. So does storing a
  set for a dead entity.
- **Explicit sets** do not follow the observer's key. One that should still
  see its own key holds it.

```rust
world.set_observer_keys::<Stealth>(scout, [0, 1, 2, 3]); // detects tier 3
world.observe_all::<Faction>(gm);
world.observe_own_key::<Stealth>(scout);                 // back to the default
```

## The audience table

`world.audience::<A>()` borrows `A`'s `AudienceTable` for reading:

| Method | Gives |
|---|---|
| `key(entity)` | The entity's key, or `None` |
| `observed(observer)` | The observer's set, as an `Observed` |
| `observes(observer, &key)` | Whether the observer's set holds `key` |
| `is_changed(entity)` | Whether the entity's key was set this tick |
| `entities()`, `iter()` | The entities holding a key, and `(Entity, Key)` pairs, in dense order |
| `len()`, `is_empty()` | The number of entities holding a key |

`Observed` is one of:

| Variant | Set |
|---|---|
| `Own(Option<K>)` | The default: the observer's own key |
| `Keys(&HashSet<K>)` | A stored set |
| `All` | Every key |

`observed.contains(&key)` checks a key against any of them.

## Tracking

An audience's table tracks its keys as a [`Tracked`](change-tracking.md#tracked-tables)
table tracks values, so key changes are found from dirty bits:

- **Dirty bits.** Setting a key marks the entry dirty, and a new key added
  and dirty.
- **Clear log.** Clearing a key logs the entity, unless the key was added
  this tick, so the two cancel out.
- **Start keys.** The table keeps the key each changed entity held at the
  start of the tick, so the entity can leave the old key's sub-frame.
- **Changes.** The table logs every change of the tick in order, with the
  key it replaced, for hooks that run after the change.
- **Observer changes.** The table logs the observers whose stored set
  changed this tick. They travel on the observer's owner channel.
- **Owned only.** Keys received from their owner are untracked, so a
  replica's key changes mark nothing and log nothing. A despawn logs
  nothing either.
- **End of tick.** The `World` clears the bits, the logs and the start keys
  when the tick ends.

## Halo stream

Neighbor shards receive entities' keys in the `keys` list of the
`internal` sub-frame, as [Frames](../protocol/frames.md#audience-keys) lays
out. Observer sets do not travel there. The runtime drives each audience's
table through `KeyTable`, without generics:

| Method | Does |
|---|---|
| `wire_id()` | The audience's `WIRE_ID` |
| `changed(&mut out)` | Appends the entities whose key was set this tick, in dense order |
| `cleared()` | The clear log. Some entities may be dead, or hold a key again |
| `encode_key(entity, &mut out)` | Appends the entity's key, and returns whether it holds one |
| `encode_start_key(entity, &mut out)` | Appends the key it held at the start of the tick, and returns whether it held one |
| `encode_keys(set, cleared, &mut out)` | Appends the audience's entry of the `keys` list, and returns whether it wrote one |
| `apply_keys(buf, resolve)` | Applies one entry, and returns its length |
| `end_tick()` | Clears the tick's changes |
| `despawn_batch(entities)` | Drops despawned entities' keys and sets without logging them |

`encode_key` and `encode_start_key` give the key half of a
[sub-frame id](../protocol/frames.md#sub-frames).

`encode_keys` writes `id:u16 | set | cleared`. **`set`** lists the
`(uid, entity)` pairs whose key is sent, such as the entities whose key
changed and those that entered `internal`, and becomes packed
`uid | key` items. Entities holding no key are skipped. **`cleared`**
lists the uids of the logged entities that are alive and hold no key
again. An entity must not be in both. Nothing is written when both parts
are empty, and nothing is left behind when a key does not encode.

`apply_keys` reads the entry after its audience id: `set` first, then
`cleared`. For each item it calls `resolve(uid)`, which names the entity
to apply it to, or returns `None` to skip it. Keys are set and cleared
untracked. A truncated part, a key `decode_key` rejects, or a zero
incarnation is an error.

```rust
// on the sim, at the end of the tick, with `table: &dyn KeyTable`
let mut changed = Vec::new();
table.changed(&mut changed);
// add the entities that entered, name them by uid, then:
table.encode_keys(&set, &cleared, &mut keys)?;

// on the neighbor shard, with `table: &mut dyn KeyTable`
let read = table.apply_keys(&entry[2..], &mut |uid| world.resolve(uid))?;
```
