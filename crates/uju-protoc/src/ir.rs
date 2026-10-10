mod lower;

use std::fmt;

pub use lower::lower;

/// Index of a [`Namespace`] in [`Schema::namespaces`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NamespaceId(pub usize);

/// Index of a [`Def`] in [`Schema::defs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub usize);

/// A set of schema sources lowered into one unit. Sources sharing a
/// `namespace` are merged into a single [`Namespace`].
#[derive(Debug, Clone, PartialEq)]
pub struct Schema {
    pub namespaces: Vec<Namespace>,
    pub defs: Vec<Def>,
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

/// The unsigned integer type backing an enum.
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

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub direction: Direction,
    pub fields: Vec<Field>,
}

/// Which way a message travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    In,
    Out,
    Bi,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: Type,
    pub optional: bool,
}

/// A type along with the constraints its values must satisfy.
#[derive(Debug, Clone, PartialEq)]
pub struct Type {
    pub kind: TypeKind,
    pub constraints: Constraints,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeKind {
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

/// Constraints on a value, checked by the validation step. Which apply
/// depends on the type: `min`/`max` on numbers, `infinite`/`nan` on floats,
/// `min_length`/`max_length` on strings, bytes, and variable-size
/// collections.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Constraints {
    /// Inclusive lower bound of a number.
    pub min: Option<Number>,
    /// Inclusive upper bound of a number.
    pub max: Option<Number>,
    /// Inclusive lower bound on the length of a string, bytes, or collection.
    pub min_length: Option<u64>,
    /// Inclusive upper bound on the length of a string, bytes, or collection.
    pub max_length: Option<u64>,
    /// Whether a float may be infinite; floats are finite by default.
    pub infinite: bool,
    /// Whether a float may be NaN; floats are not NaN by default.
    pub nan: bool,
}

/// A bound, typed after the number it constrains.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Number {
    Int(i128),
    Float(f64),
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

impl From<TypeKind> for Type {
    fn from(kind: TypeKind) -> Self {
        Type {
            kind,
            constraints: Constraints::default(),
        }
    }
}

impl Constraints {
    pub fn is_empty(&self) -> bool {
        *self == Constraints::default()
    }
}

impl Direction {
    pub fn name(self) -> &'static str {
        match self {
            Direction::In => "in",
            Direction::Out => "out",
            Direction::Bi => "bi",
        }
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

/// Prints as written in source, `[min=0, max=10]`, or nothing when empty.
impl fmt::Display for Constraints {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut entries = Vec::new();
        if let Some(min) = self.min {
            entries.push(format!("min={min}"));
        }
        if let Some(max) = self.max {
            entries.push(format!("max={max}"));
        }
        if let Some(min_length) = self.min_length {
            entries.push(format!("min_length={min_length}"));
        }
        if let Some(max_length) = self.max_length {
            entries.push(format!("max_length={max_length}"));
        }
        if self.infinite {
            entries.push("infinite".to_string());
        }
        if self.nan {
            entries.push("nan".to_string());
        }
        if entries.is_empty() {
            return Ok(());
        }
        write!(f, "[{}]", entries.join(", "))
    }
}

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Number::Int(value) => write!(f, "{value}"),
            // `{:?}` keeps the fraction of whole floats: `1.0`, not `1`.
            Number::Float(value) => write!(f, "{value:?}"),
        }
    }
}
