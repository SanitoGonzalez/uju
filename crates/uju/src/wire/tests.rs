use core::cmp::Ordering;
use core::time::Duration;

use chrono::{DateTime, Utc};

use crate::ecs::entity::{Entity, EntityGeneration, EntityIndex, UniversalEntity};
use crate::wire::*;

fn entity(index: u32, generation: u32) -> Entity {
    Entity::new(EntityIndex::new(index), EntityGeneration::new(generation))
}

fn roundtrip<'a, T>(value: &T, bytes: &'a mut Vec<u8>) -> <T::View as View<'a>>::Owned
where
    T: Wire + Viewed,
{
    *bytes = encode(value).unwrap();
    assert_eq!(bytes.len(), value.encoded_size(), "encoded_size mismatch");
    let consumed = T::View::validate(bytes).unwrap();
    assert_eq!(consumed, bytes.len(), "validate consumed the wrong length");
    T::View::read(bytes).owned()
}

/// Ties an owned type to the view that decodes it, so `roundtrip` can be generic.
trait Viewed {
    type View: for<'a> View<'a>;
}

macro_rules! viewed {
    ($owned:ty, $view:ty) => {
        impl Viewed for $owned {
            type View = $view;
        }
    };
}

viewed!(u8, u8);
viewed!(u16, u16);
viewed!(u32, u32);
viewed!(u64, u64);
viewed!(i8, i8);
viewed!(i16, i16);
viewed!(i32, i32);
viewed!(i64, i64);
viewed!(f32, f32);
viewed!(f64, f64);
viewed!(bool, bool);
viewed!(Timestamp, Timestamp);
viewed!(Interval, Interval);
viewed!(Entity, Entity);
viewed!(UniversalEntity, UniversalEntity);

macro_rules! assert_roundtrip {
    ($value:expr, $size:expr) => {{
        let value = $value;
        let mut bytes = Vec::new();
        let back = roundtrip(&value, &mut bytes);
        assert_eq!(bytes.len(), $size);
        assert_eq!(back, value);
        bytes
    }};
}

#[test]
fn scalars_roundtrip() {
    assert_roundtrip!(0x12u8, 1);
    assert_roundtrip!(0x1234u16, 2);
    assert_roundtrip!(0x1234_5678u32, 4);
    assert_roundtrip!(0x1234_5678_9abc_def0u64, 8);
    assert_roundtrip!(-3i8, 1);
    assert_roundtrip!(-300i16, 2);
    assert_roundtrip!(i32::MIN, 4);
    assert_roundtrip!(i64::MIN, 8);
    assert_roundtrip!(true, 1);
    assert_roundtrip!(false, 1);
    assert_roundtrip!(-0.5f32, 4);
    assert_roundtrip!(f64::MAX, 8);
}

#[test]
fn scalars_are_little_endian() {
    assert_eq!(encode(&0x1234u16).unwrap(), [0x34, 0x12]);
    assert_eq!(encode(&0x1234_5678u32).unwrap(), [0x78, 0x56, 0x34, 0x12]);
    assert_eq!(encode(&true).unwrap(), [1]);
    assert_eq!(encode(&false).unwrap(), [0]);
}

#[test]
fn bool_rejects_other_values() {
    assert_eq!(<bool as View>::validate(&[2]), Err(Error::BadBool));
    assert_eq!(<bool as View>::validate(&[]), Err(Error::Truncated));
}

#[test]
fn timestamp_roundtrips_in_microseconds() {
    let value = DateTime::from_timestamp_micros(1_700_000_000_123_456).unwrap();
    let bytes = assert_roundtrip!(value, 8);
    assert_eq!(bytes, 1_700_000_000_123_456i64.to_le_bytes());

    assert_roundtrip!(DateTime::<Utc>::from_timestamp_micros(0).unwrap(), 8);
    assert_roundtrip!(DateTime::<Utc>::from_timestamp_micros(-1).unwrap(), 8);
}

#[test]
fn timestamp_rejects_out_of_range() {
    let bytes = i64::MAX.to_le_bytes();
    assert_eq!(
        <Timestamp as View>::validate(&bytes),
        Err(Error::BadTimestamp)
    );
}

#[test]
fn interval_roundtrips_in_microseconds() {
    let bytes = assert_roundtrip!(Duration::from_micros(1_500_000), 8);
    assert_eq!(bytes, 1_500_000i64.to_le_bytes());
    assert_roundtrip!(Duration::ZERO, 8);

    // Sub-microsecond precision is not representable on the wire.
    assert_eq!(
        encode(&Duration::from_nanos(1500)).unwrap(),
        1i64.to_le_bytes()
    );
}

#[test]
fn interval_rejects_negative() {
    let bytes = (-1i64).to_le_bytes();
    assert_eq!(
        <Interval as View>::validate(&bytes),
        Err(Error::BadInterval)
    );
}

#[test]
fn interval_rejects_unencodable_duration() {
    assert_eq!(encode(&Duration::MAX), Err(Error::TooLarge));
}

#[test]
fn entity_roundtrips() {
    let bytes = assert_roundtrip!(entity(7, 3), 8);
    assert_eq!(bytes, [7, 0, 0, 0, 3, 0, 0, 0]);
}

#[test]
fn entity_rejects_null_index() {
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(<Entity as View>::validate(&bytes), Err(Error::BadEntity));
}

#[test]
fn universal_entity_roundtrips() {
    let value = UniversalEntity::new(2, 5, entity(7, 3));
    let bytes = assert_roundtrip!(value, 12);
    assert_eq!(bytes, [2, 0, 5, 0, 7, 0, 0, 0, 3, 0, 0, 0]);
}

#[test]
fn universal_entity_rejects_null_index() {
    let mut bytes = [0u8; 12];
    bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        <UniversalEntity as View>::validate(&bytes),
        Err(Error::BadEntity)
    );
}

#[test]
fn string_roundtrips() {
    let value = "héllo".to_string();
    let bytes = encode(&value).unwrap();
    assert_eq!(bytes.len(), 2 + 6);
    assert_eq!(&bytes[..2], 6u16.to_le_bytes());
    assert_eq!(<&str as View>::validate(&bytes), Ok(8));
    assert_eq!(<&str as View>::read(&bytes), "héllo");

    let empty = encode(&String::new()).unwrap();
    assert_eq!(empty, [0, 0]);
    assert_eq!(<&str as View>::validate(&empty), Ok(2));
}

#[test]
fn string_rejects_bad_utf8() {
    let mut bytes = encode(&"abc".to_string()).unwrap();
    bytes[2] = 0xff;
    assert_eq!(<&str as View>::validate(&bytes), Err(Error::BadUtf8));
}

#[test]
fn string_rejects_truncation() {
    let bytes = encode(&"abc".to_string()).unwrap();
    assert_eq!(<&str as View>::validate(&bytes[..4]), Err(Error::Truncated));
    assert_eq!(<&str as View>::validate(&bytes[..1]), Err(Error::Truncated));
}

#[test]
fn bytes_roundtrip() {
    let value: Vec<u8> = vec![1, 2, 3];
    let bytes = encode(&value).unwrap();
    assert_eq!(bytes, [3, 0, 1, 2, 3]);
    assert_eq!(<&[u8] as View>::validate(&bytes), Ok(5));
    assert_eq!(<&[u8] as View>::read(&bytes), &[1, 2, 3]);
}

#[test]
fn vec_of_fixed_elements() {
    let value = vec![1i32, 2, 3];
    let bytes = encode(&value).unwrap();
    assert_eq!(bytes.len(), 2 + 12);
    assert_eq!(value.encoded_size(), bytes.len());
    assert_eq!(VecView::<i32>::validate(&bytes), Ok(14));

    let view = VecView::<i32>::read(&bytes);
    assert_eq!(view.len(), 3);
    assert_eq!(view.get(1), Some(2));
    assert_eq!(view.get(3), None);
    assert_eq!(view.iter().collect::<Vec<_>>(), value);
    assert_eq!(view.owned(), value);
}

#[test]
fn vec_of_variable_elements() {
    let value = vec!["a".to_string(), "bcd".to_string()];
    let bytes = encode(&value).unwrap();
    // count + 2 offsets + (2 + 1) + (2 + 3)
    assert_eq!(bytes.len(), 2 + 4 + 3 + 5);
    assert_eq!(value.encoded_size(), bytes.len());
    assert_eq!(VecView::<&str>::validate(&bytes), Ok(bytes.len()));

    let view = VecView::<&str>::read(&bytes);
    assert_eq!(view.iter().collect::<Vec<_>>(), ["a", "bcd"]);
    assert_eq!(view.owned(), value);
}

#[test]
fn nested_vec() {
    let value = vec![vec![1i32, 2], vec![], vec![3i32]];
    let bytes = encode(&value).unwrap();
    assert_eq!(value.encoded_size(), bytes.len());
    assert_eq!(VecView::<VecView<i32>>::validate(&bytes), Ok(bytes.len()));
    assert_eq!(VecView::<VecView<i32>>::read(&bytes).owned(), value);
}

#[test]
fn empty_vec() {
    let value: Vec<i32> = Vec::new();
    let bytes = encode(&value).unwrap();
    assert_eq!(bytes, [0, 0]);
    assert_eq!(VecView::<i32>::validate(&bytes), Ok(2));
    assert!(VecView::<i32>::read(&bytes).is_empty());
}

#[test]
fn vec_rejects_non_sequential_offsets() {
    let value = vec!["a".to_string(), "bcd".to_string()];
    let mut bytes = encode(&value).unwrap();
    bytes[2] += 1;
    assert_eq!(VecView::<&str>::validate(&bytes), Err(Error::BadOffset));
}

#[test]
fn vec_rejects_truncation() {
    let bytes = encode(&vec![1i32, 2, 3]).unwrap();
    assert_eq!(
        VecView::<i32>::validate(&bytes[..bytes.len() - 1]),
        Err(Error::Truncated)
    );
}

#[test]
fn set_sorts_and_deduplicates() {
    let set = Set::from_vec(vec![3i32, 1, 2, 1]);
    assert_eq!(set.as_slice(), [1, 2, 3]);
    assert!(set.contains(&2));
    assert!(!set.contains(&4));

    let bytes = encode(&set).unwrap();
    assert_eq!(set.encoded_size(), bytes.len());
    assert_eq!(SetView::<i32>::validate(&bytes), Ok(bytes.len()));

    let view = SetView::<i32>::read(&bytes);
    assert!(view.contains(&3));
    assert!(!view.contains(&0));
    assert_eq!(view.owned(), set);
}

#[test]
fn set_insert_keeps_order() {
    let mut set = Set::new();
    assert!(set.insert(5i32));
    assert!(set.insert(1i32));
    assert!(!set.insert(5i32));
    assert_eq!(set.as_slice(), [1, 5]);
}

#[test]
fn set_rejects_unsorted_and_duplicate() {
    let bytes = encode(&Set::from_vec(vec![1i32, 2])).unwrap();

    let mut unsorted = bytes.clone();
    unsorted[2..6].copy_from_slice(&2i32.to_le_bytes());
    unsorted[6..10].copy_from_slice(&1i32.to_le_bytes());
    assert_eq!(SetView::<i32>::validate(&unsorted), Err(Error::Unsorted));

    let mut duplicate = bytes.clone();
    duplicate[6..10].copy_from_slice(&1i32.to_le_bytes());
    assert_eq!(SetView::<i32>::validate(&duplicate), Err(Error::Duplicate));
}

#[test]
fn map_roundtrips_with_fixed_columns() {
    let map = Map::from_vec(vec![(2u32, 20i32), (1u32, 10i32)]);
    assert_eq!(map.get(&1), Some(&10));
    assert_eq!(map.get(&3), None);

    let bytes = encode(&map).unwrap();
    assert_eq!(map.encoded_size(), bytes.len());
    // count + values_start + 2 keys + 2 values
    assert_eq!(bytes.len(), 4 + 8 + 8);
    assert_eq!(read_u16(&bytes, 0), 2);
    assert_eq!(read_u16(&bytes, 2), 12, "values_start");
    assert_eq!(MapView::<u32, i32>::validate(&bytes), Ok(bytes.len()));

    let view = MapView::<u32, i32>::read(&bytes);
    assert_eq!(view.len(), 2);
    assert_eq!(view.key(0), Some(1));
    assert_eq!(view.get(&2), Some(20));
    assert_eq!(view.get(&9), None);
    assert_eq!(view.keys().collect::<Vec<_>>(), [1, 2]);
    assert_eq!(view.values().collect::<Vec<_>>(), [10, 20]);
    assert_eq!(view.owned(), map);
}

#[test]
fn map_roundtrips_with_variable_columns() {
    let map = Map::from_vec(vec![
        ("bb".to_string(), vec![1i32, 2]),
        ("a".to_string(), vec![]),
    ]);
    let bytes = encode(&map).unwrap();
    assert_eq!(map.encoded_size(), bytes.len());
    assert_eq!(
        MapView::<&str, VecView<i32>>::validate(&bytes),
        Ok(bytes.len())
    );

    let view = MapView::<&str, VecView<i32>>::read(&bytes);
    assert_eq!(view.get(&"bb").map(View::owned), Some(vec![1, 2]));
    assert_eq!(view.get(&"a").map(View::owned), Some(vec![]));
    assert_eq!(view.owned(), map);
}

#[test]
fn empty_map() {
    let map: Map<u32, i32> = Map::new();
    let bytes = encode(&map).unwrap();
    assert_eq!(bytes, [0, 0, 4, 0]);
    assert_eq!(MapView::<u32, i32>::validate(&bytes), Ok(4));
    assert!(MapView::<u32, i32>::read(&bytes).is_empty());
}

#[test]
fn map_rejects_bad_values_start() {
    let map = Map::from_vec(vec![(1u32, 10i32), (2u32, 20i32)]);
    let mut bytes = encode(&map).unwrap();
    bytes[2] += 1;
    assert_eq!(MapView::<u32, i32>::validate(&bytes), Err(Error::BadOffset));
}

#[test]
fn map_rejects_unsorted_keys() {
    let map = Map::from_vec(vec![(1u32, 10i32), (2u32, 20i32)]);
    let mut bytes = encode(&map).unwrap();
    bytes[4..8].copy_from_slice(&2u32.to_le_bytes());
    bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(MapView::<u32, i32>::validate(&bytes), Err(Error::Unsorted));
}

#[test]
fn map_rejects_duplicate_keys() {
    let map = Map::from_vec(vec![(1u32, 10i32), (2u32, 20i32)]);
    let mut bytes = encode(&map).unwrap();
    bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(MapView::<u32, i32>::validate(&bytes), Err(Error::Duplicate));
}

#[test]
fn encoding_is_canonical() {
    let a = Set::from_vec(vec![3i32, 1, 2]);
    let b: Set<i32> = [2i32, 3, 1].into_iter().collect();
    assert_eq!(encode(&a).unwrap(), encode(&b).unwrap());
}

#[test]
fn canonical_order_of_floats_is_total() {
    assert_eq!(f32::NAN.canonical_cmp(&f32::INFINITY), Ordering::Greater);
    assert_eq!((-0.0f64).canonical_cmp(&0.0), Ordering::Less);
    assert_eq!(1.0f64.canonical_cmp(&1.0), Ordering::Equal);
}

#[test]
fn canonical_order_of_strings_is_bytewise() {
    assert_eq!("ab".canonical_cmp(&"abc"), Ordering::Less);
    assert_eq!("b".canonical_cmp(&"ab"), Ordering::Greater);
}

#[test]
fn canonical_order_of_options_puts_absent_first() {
    assert_eq!(None::<i32>.canonical_cmp(&Some(0)), Ordering::Less);
    assert_eq!(Some(1).canonical_cmp(&None), Ordering::Greater);
    assert_eq!(None::<i32>.canonical_cmp(&None), Ordering::Equal);
}

#[test]
fn canonical_order_of_containers_is_elementwise_then_length() {
    assert_eq!(vec![1i32, 2].canonical_cmp(&vec![1, 3]), Ordering::Less);
    assert_eq!(vec![1i32].canonical_cmp(&vec![1, 0]), Ordering::Less);
    assert_eq!(vec![1i32, 2].canonical_cmp(&vec![1, 2]), Ordering::Equal);
}

#[test]
fn canonical_order_of_entities_is_fieldwise() {
    assert_eq!(
        entity(1, 9).canonical_cmp(&entity(2, 0)),
        Ordering::Less,
        "index dominates generation"
    );
    assert_eq!(entity(1, 0).canonical_cmp(&entity(1, 1)), Ordering::Less);
    assert_eq!(
        UniversalEntity::new(1, 0, entity(9, 0)).canonical_cmp(&UniversalEntity::new(
            0,
            9,
            entity(0, 0)
        )),
        Ordering::Greater,
        "node dominates shard and entity"
    );
}

#[test]
fn encode_into_reports_a_short_buffer() {
    let mut buf = [0u8; 3];
    assert_eq!(encode_into(&1u32, &mut buf), Err(Error::Truncated));
    let mut buf = [0u8; 4];
    assert_eq!(encode_into(&1u32, &mut buf), Ok(4));
}

#[test]
fn oversized_container_is_rejected() {
    let value = vec![0u8; u16::MAX as usize + 1];
    assert_eq!(encode(&value), Err(Error::TooLarge));
}
