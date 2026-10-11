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
    /// Inclusive lower bound on the length of a string or bytes (in bytes) or
    /// of a collection (in items).
    pub min_length: Option<u64>,
    /// Inclusive upper bound on the length of a string or bytes (in bytes) or
    /// of a collection (in items).
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

    /// A declaration's namespace, the declarations it is nested in, and its
    /// own name, joined with `.`: `foo.bar.MyResponse.Result`.
    pub fn qualified(&self, id: TypeId) -> String {
        let mut name = self.namespace(self.def(id).namespace).name.join(".");
        for segment in self.path(id) {
            name.push('.');
            name.push_str(segment);
        }
        name
    }

    /// The type as it would be written in source, with named types qualified
    /// and constraints in canonical order: `vec<foo.bar.Position>[max_length=8]`.
    pub fn type_name(&self, ty: &Type) -> String {
        let name = match &ty.kind {
            TypeKind::I8 => "i8".to_string(),
            TypeKind::I16 => "i16".to_string(),
            TypeKind::I32 => "i32".to_string(),
            TypeKind::I64 => "i64".to_string(),
            TypeKind::U8 => "u8".to_string(),
            TypeKind::U16 => "u16".to_string(),
            TypeKind::U32 => "u32".to_string(),
            TypeKind::U64 => "u64".to_string(),
            TypeKind::F32 => "f32".to_string(),
            TypeKind::F64 => "f64".to_string(),
            TypeKind::Bool => "bool".to_string(),
            TypeKind::Timestamp => "timestamp".to_string(),
            TypeKind::Interval => "interval".to_string(),
            TypeKind::Entity => "entity".to_string(),
            TypeKind::UEntity => "uentity".to_string(),
            TypeKind::Uuid => "uuid".to_string(),
            TypeKind::String => "string".to_string(),
            TypeKind::Bytes => "bytes".to_string(),
            TypeKind::Array(item, len) => format!("array<{}, {len}>", self.type_name(item)),
            TypeKind::Vec(item) => format!("vec<{}>", self.type_name(item)),
            TypeKind::Set(item) => format!("set<{}>", self.type_name(item)),
            TypeKind::Map(key, value) => {
                format!("map<{}, {}>", self.type_name(key), self.type_name(value))
            }
            TypeKind::Named(id) => self.qualified(*id),
        };
        format!("{name}{}", ty.constraints)
    }

    /// Whether every value of `ty` encodes to the same number of bytes. A
    /// struct is fixed-size when all of its fields are, and none is optional
    /// but an `option<bool>`, which takes two bits either way.
    pub fn is_fixed(&self, ty: &Type) -> bool {
        self.is_fixed_in(ty, &mut Vec::new())
    }

    /// `visiting` guards against recursive structs, which are reported
    /// separately.
    fn is_fixed_in(&self, ty: &Type, visiting: &mut Vec<TypeId>) -> bool {
        match &ty.kind {
            TypeKind::String | TypeKind::Bytes => ty.constraints.fixed_length().is_some(),
            TypeKind::Vec(_) | TypeKind::Set(_) | TypeKind::Map(..) => false,
            TypeKind::Array(item, _) => self.is_fixed_in(item, visiting),
            TypeKind::Named(id) => match &self.def(*id).kind {
                DefKind::Enum(_) => true,
                DefKind::Message(_) => false,
                DefKind::Struct(item) => {
                    if visiting.contains(id) {
                        return true;
                    }
                    visiting.push(*id);
                    let fixed = item.fields.iter().all(|field| {
                        (!field.optional || field.ty.kind == TypeKind::Bool)
                            && self.is_fixed_in(&field.ty, visiting)
                    });
                    visiting.pop();
                    fixed
                }
            },
            _ => true,
        }
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

    /// The length every value has, when `min_length` and `max_length` agree:
    /// `string[min_length=3, max_length=3]` is always 3 bytes.
    pub fn fixed_length(&self) -> Option<u64> {
        self.min_length.filter(|min| Some(*min) == self.max_length)
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
