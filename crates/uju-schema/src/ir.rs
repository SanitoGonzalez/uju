mod lower;
pub mod wire;

use std::fmt;

pub use lower::lower;

/// Index of a [`Namespace`] in [`Schema::namespaces`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NamespaceId(pub usize);

/// Index of a [`Def`] in [`Schema::defs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub usize);

/// Index of a [`Const`] in [`Schema::consts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConstId(pub usize);

/// A set of schema sources lowered into one unit. Sources sharing a
/// `namespace` are merged into a single [`Namespace`].
#[derive(Debug, Clone, PartialEq)]
pub struct Schema {
    pub namespaces: Vec<Namespace>,
    pub defs: Vec<Def>,
    pub consts: Vec<Const>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Namespace {
    pub name: Vec<String>,
}

/// A named type declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct Def {
    pub namespace: NamespaceId,
    /// The declaration this one is nested in, if any.
    pub parent: Option<TypeId>,
    pub name: String,
    pub kind: DefKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DefKind {
    Enum(Enum),
    Struct(Struct),
    Union(Union),
    Message(Message),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enum {
    pub repr: EnumRepr,
    pub variants: Vec<Variant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub name: String,
    pub value: u64,
}

/// The unsigned integer type backing an enum on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnumRepr {
    U8,
    U16,
    U32,
    U64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Struct {
    pub fields: Vec<Field>,
}

/// A tagged choice; exactly one member is set at a time.
#[derive(Debug, Clone, PartialEq)]
pub struct Union {
    pub members: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub response: Option<TypeId>,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: Type,
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
    Array(Box<Type>, u32),
    Vec(Box<Type>),
    Map(Box<Type>, Box<Type>),
    Set(Box<Type>),
    Named(TypeId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Const {
    pub namespace: NamespaceId,
    /// The declaration this one is nested in, if any.
    pub parent: Option<TypeId>,
    pub name: String,
    pub ty: Type,
    pub value: Value,
}

/// An evaluated constant.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i128),
    Float(f64),
    /// Strings are stored with their escapes decoded.
    Str(String),
    /// A variant of the enum, by index into its variant list.
    EnumVariant(TypeId, usize),
}

impl Schema {
    pub fn def(&self, id: TypeId) -> &Def {
        &self.defs[id.0]
    }

    pub fn namespace(&self, id: NamespaceId) -> &Namespace {
        &self.namespaces[id.0]
    }

    /// The names leading to `id` within its namespace, outermost first, such
    /// as `["MyResponse", "Result"]` for an enum nested in a message.
    pub fn path(&self, id: TypeId) -> Vec<&str> {
        let mut path = Vec::new();
        let mut cursor = Some(id);
        while let Some(id) = cursor {
            let def = self.def(id);
            path.push(def.name.as_str());
            cursor = def.parent;
        }
        path.reverse();
        path
    }
}

impl EnumRepr {
    pub fn max(self) -> u64 {
        match self {
            EnumRepr::U8 => u8::MAX as u64,
            EnumRepr::U16 => u16::MAX as u64,
            EnumRepr::U32 => u32::MAX as u64,
            EnumRepr::U64 => u64::MAX,
        }
    }

    /// Encoded size in bytes.
    pub fn size(self) -> usize {
        match self {
            EnumRepr::U8 => 1,
            EnumRepr::U16 => 2,
            EnumRepr::U32 => 4,
            EnumRepr::U64 => 8,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            EnumRepr::U8 => "u8",
            EnumRepr::U16 => "u16",
            EnumRepr::U32 => "u32",
            EnumRepr::U64 => "u64",
        }
    }
}

impl fmt::Display for EnumRepr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
