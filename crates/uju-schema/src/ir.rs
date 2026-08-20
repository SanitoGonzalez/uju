mod lower;
pub mod wire;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub usize);

pub enum Type {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Bool,
    Timestamp,
    Interval,
    Entity,
    UEntity,
    Uuid,
    String,
    Bytes,
    Array(Box<Type>, usize),
    Vec(Box<Type>),
    Map(Box<Type>, Box<Type>),
    Set(Box<Type>),
    Named(TypeId),
}
