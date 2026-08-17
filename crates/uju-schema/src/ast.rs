use chumsky::span::SimpleSpan;

pub type Span = SimpleSpan;
pub type Ident = Spanned<String>;

#[derive(Debug, Clone, PartialEq)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub fn new(node: T, span: Span) -> Self {
        Self { node, span }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Prim {
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
    String,
    Bytes,
}

impl Prim {
    pub fn name(self) -> &'static str {
        match self {
            Prim::I8 => "i8",
            Prim::I16 => "i16",
            Prim::I32 => "i32",
            Prim::I64 => "i64",
            Prim::U8 => "u8",
            Prim::U16 => "u16",
            Prim::U32 => "u32",
            Prim::U64 => "u64",
            Prim::F32 => "f32",
            Prim::F64 => "f64",
            Prim::Bool => "bool",
            Prim::Timestamp => "timestamp",
            Prim::Interval => "interval",
            Prim::Entity => "entity",
            Prim::UEntity => "uentity",
            Prim::String => "string",
            Prim::Bytes => "bytes",
        }
    }

    pub fn is_integer(self) -> bool {
        matches!(
            self,
            Prim::I8
                | Prim::I16
                | Prim::I32
                | Prim::I64
                | Prim::U8
                | Prim::U16
                | Prim::U32
                | Prim::U64
        )
    }

    pub fn is_unsigned(self) -> bool {
        matches!(self, Prim::U8 | Prim::U16 | Prim::U32 | Prim::U64)
    }

    pub fn is_fixed_size(self) -> bool {
        self.size().is_some()
    }

    pub fn size(self) -> Option<u32> {
        match self {
            Prim::I8 | Prim::U8 | Prim::Bool => Some(1),
            Prim::I16 | Prim::U16 => Some(2),
            Prim::I32 | Prim::U32 | Prim::F32 => Some(4),
            Prim::I64 | Prim::U64 | Prim::F64 | Prim::Timestamp | Prim::Interval | Prim::Entity => {
                Some(8)
            }
            Prim::UEntity => Some(12),
            Prim::String | Prim::Bytes => None,
        }
    }
}

impl core::fmt::Display for Prim {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Schema {
    pub namespace: Option<Path>,
    pub uses: Vec<Path>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Path(pub Vec<Ident>);

impl Path {
    pub fn span(&self) -> Span {
        let start = self.0.first().map(|i| i.span.start).unwrap_or(0);
        let end = self.0.last().map(|i| i.span.end).unwrap_or(start);
        (start..end).into()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Const(Const),
    Enum(Enum),
    Struct(Struct),
    Message(Message),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Const {
    pub name: Ident,
    pub ty: Spanned<TypeRef>,
    pub value: Spanned<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Enum {
    pub name: Ident,
    pub repr: Option<Spanned<Prim>>,
    pub variants: Vec<Variant>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub name: Ident,
    pub value: Option<Spanned<i64>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructKind {
    Struct,
    Component,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Struct {
    pub kind: StructKind,
    pub name: Ident,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub name: Ident,
    pub returns: Option<Path>,
    pub items: Vec<Item>,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: Ident,
    pub ty: Spanned<TypeRef>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeRef {
    Prim(Prim),
    Named(Path),
    Vec(Box<Spanned<TypeRef>>),
    Set(Box<Spanned<TypeRef>>),
    Map(Box<Spanned<TypeRef>>, Box<Spanned<TypeRef>>),
    Optional(Box<Spanned<TypeRef>>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Path(Path),
}
