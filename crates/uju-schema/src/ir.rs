pub use crate::ast::Prim;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Fixed(u32),
    Variable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Schema {
    pub namespace: Vec<String>,
    pub types: Vec<TypeDef>,
    pub consts: Vec<ConstDef>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeDef {
    Record(RecordDef),
    Enum(EnumDef),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    Struct,
    Component,
    Message,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordDef {
    pub name: String,
    pub kind: RecordKind,
    pub fields: Vec<FieldDef>,
    pub layout: Layout,
    pub message: Option<MessageInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageInfo {
    pub id: u32,
    pub returns: Option<TypeId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub bitmap_bytes: u32,
    pub fixed_size: u32,
    pub size: Size,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldDef {
    pub name: String,
    pub ty: Ty,
    pub optional: bool,
    pub offset: u32,
    pub bit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumDef {
    pub name: String,
    pub repr: Prim,
    pub variants: Vec<VariantDef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VariantDef {
    pub name: String,
    pub value: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConstDef {
    pub name: String,
    pub ty: Ty,
    pub value: ConstValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Variant { ty: TypeId, index: u32 },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Prim(Prim),
    Ref(TypeId),
    Vec(Box<Ty>),
    Set(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
}

impl Schema {
    pub fn type_def(&self, id: TypeId) -> &TypeDef {
        &self.types[id.0 as usize]
    }

    pub fn type_id(&self, name: &str) -> Option<TypeId> {
        self.types
            .iter()
            .position(|t| t.name() == name)
            .map(|i| TypeId(i as u32))
    }
}

impl TypeDef {
    pub fn name(&self) -> &str {
        match self {
            TypeDef::Record(def) => &def.name,
            TypeDef::Enum(def) => &def.name,
        }
    }

    pub fn size(&self) -> Size {
        match self {
            TypeDef::Record(def) => def.layout.size,
            TypeDef::Enum(def) => Size::Fixed(def.repr.size().unwrap()),
        }
    }
}

impl Ty {
    pub fn size(&self, schema: &Schema) -> Size {
        match self {
            Ty::Prim(p) => p.size().map(Size::Fixed).unwrap_or(Size::Variable),
            Ty::Ref(id) => schema.type_def(*id).size(),
            Ty::Vec(_) | Ty::Set(_) | Ty::Map(_, _) => Size::Variable,
        }
    }

    pub fn is_container(&self) -> bool {
        matches!(self, Ty::Vec(_) | Ty::Set(_) | Ty::Map(_, _))
    }
}
