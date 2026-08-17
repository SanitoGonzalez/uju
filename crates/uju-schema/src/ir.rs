use core::fmt::Write as _;

pub use crate::ast::Prim;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Fixed(u32),
    Variable,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Name {
    pub namespace: Vec<String>,
    pub scope: Vec<String>,
    pub name: String,
}

impl Name {
    pub fn qualified(&self) -> String {
        let mut out = String::new();
        for part in self.namespace.iter().chain(&self.scope) {
            out.push_str(part);
            out.push('.');
        }
        out.push_str(&self.name);
        out
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Schema {
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
    pub name: Name,
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
    pub name: Name,
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
    pub name: Name,
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

    pub fn record(&self, id: TypeId) -> Option<&RecordDef> {
        match self.type_def(id) {
            TypeDef::Record(def) => Some(def),
            TypeDef::Enum(_) => None,
        }
    }

    pub fn type_id(&self, qualified: &str) -> Option<TypeId> {
        self.types
            .iter()
            .position(|t| t.name().qualified() == qualified)
            .map(|i| TypeId(i as u32))
    }

    pub fn namespaces(&self) -> Vec<Vec<String>> {
        let mut out: Vec<Vec<String>> = Vec::new();
        for namespace in self
            .types
            .iter()
            .map(|t| &t.name().namespace)
            .chain(self.consts.iter().map(|c| &c.name.namespace))
        {
            if !out.contains(namespace) {
                out.push(namespace.clone());
            }
        }
        out.sort();
        out
    }

    pub fn hash(&self) -> u64 {
        let mut text = String::new();
        for def in &self.types {
            match def {
                TypeDef::Enum(def) => {
                    let _ = write!(text, "enum {}:{};", def.name.qualified(), def.repr.name());
                    for variant in &def.variants {
                        let _ = write!(text, "{}={};", variant.name, variant.value);
                    }
                }
                TypeDef::Record(def) => {
                    let _ = write!(
                        text,
                        "record {}:{:?}:{}:{};",
                        def.name.qualified(),
                        def.kind,
                        def.layout.bitmap_bytes,
                        def.layout.fixed_size
                    );
                    if let Some(info) = def.message {
                        let _ = write!(text, "id={};", info.id);
                    }
                    for field in &def.fields {
                        let _ = write!(
                            text,
                            "{}@{}{}:{};",
                            field.name,
                            field.offset,
                            if field.optional { "?" } else { "" },
                            self.ty_text(&field.ty)
                        );
                    }
                }
            }
        }
        fnv1a(text.as_bytes())
    }

    fn ty_text(&self, ty: &Ty) -> String {
        match ty {
            Ty::Prim(p) => p.name().to_string(),
            Ty::Ref(id) => self.type_def(*id).name().qualified(),
            Ty::Vec(t) => format!("vec<{}>", self.ty_text(t)),
            Ty::Set(t) => format!("set<{}>", self.ty_text(t)),
            Ty::Map(k, v) => format!("map<{},{}>", self.ty_text(k), self.ty_text(v)),
        }
    }
}

impl TypeDef {
    pub fn name(&self) -> &Name {
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

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}
