macro_rules! reader {
    ($name:ident, $ty:ty, $n:literal) => {
        #[inline]
        pub fn $name(bytes: &[u8], at: usize) -> $ty {
            <$ty>::from_le_bytes(bytes[at..at + $n].try_into().unwrap())
        }
    };
}

#[inline]
pub fn read_u8(bytes: &[u8], at: usize) -> u8 {
    bytes[at]
}

#[inline]
pub fn read_i8(bytes: &[u8], at: usize) -> i8 {
    bytes[at] as i8
}

#[inline]
pub fn read_bool(bytes: &[u8], at: usize) -> bool {
    bytes[at] != 0
}

reader!(read_u16, u16, 2);
reader!(read_u32, u32, 4);
reader!(read_u64, u64, 8);
reader!(read_i16, i16, 2);
reader!(read_i32, i32, 4);
reader!(read_i64, i64, 8);
reader!(read_f32, f32, 4);
reader!(read_f64, f64, 8);

#[inline]
pub fn test_bit(bytes: &[u8], at: usize, bit: u32) -> bool {
    bytes[at + (bit / 8) as usize] & (1 << (bit % 8)) != 0
}

#[inline]
pub fn is_zero(bytes: &[u8], at: usize, len: usize) -> bool {
    bytes[at..at + len].iter().all(|&b| b == 0)
}
