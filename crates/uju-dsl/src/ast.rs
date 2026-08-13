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
    String,
}

impl Prim {
    pub fn is_integer(self) -> bool {
        todo!()
    }

    pub fn is_fixed_size(self) -> bool {
        todo!()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Schema {
    pub namespace: Option<Path>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Path(pub Vec<Ident>);

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Const(Const),
    Enum(Enum),
    Struct(Struct),
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

#[derive(Debug, Clone, PartialEq)]
pub struct Struct {
    pub name: Ident,
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
    Array(Box<Spanned<TypeRef>>),
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
