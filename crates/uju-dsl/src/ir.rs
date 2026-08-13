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
    Struct(StructDef),
    Enum(EnumDef),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<FieldDef>,
    pub size: Size,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldDef {
    pub name: String,
    pub ty: Ty,
    pub offset: Option<u32>,
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
    pub value: i64,
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
    List(Box<Ty>),
    Option(Box<Ty>),
    Ref(TypeId),
}

impl Schema {
    pub fn type_def(&self, id: TypeId) -> &TypeDef {
        todo!()
    }

    pub fn type_id(&self, name: &str) -> Option<TypeId> {
        todo!()
    }
}

impl TypeDef {
    pub fn name(&self) -> &str {
        todo!()
    }

    pub fn size(&self) -> Size {
        todo!()
    }
}

impl Ty {
    pub fn size(&self, schema: &Schema) -> Size {
        todo!()
    }
}
