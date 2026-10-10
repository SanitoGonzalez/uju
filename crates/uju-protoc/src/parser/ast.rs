use std::fmt;

use crate::lexer::{Span, Spanned};

pub type Ident<'src> = Spanned<&'src str>;

#[derive(Debug, Clone, PartialEq)]
pub struct Schema<'src> {
    pub namespace: Path<'src>,
    pub uses: Vec<Path<'src>>,
    pub items: Vec<Item<'src>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Path<'src> {
    pub segments: Vec<Ident<'src>>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item<'src> {
    Enum(Enum<'src>),
    Struct(Struct<'src>),
    Message(Message<'src>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Enum<'src> {
    pub name: Ident<'src>,
    /// `None` when the underlying type is left to default to `u32`.
    pub repr: Option<Spanned<Prim>>,
    pub variants: Vec<Variant<'src>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variant<'src> {
    pub name: Ident<'src>,
    pub value: Option<Spanned<i128>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Struct<'src> {
    pub name: Ident<'src>,
    pub body: Body<'src>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message<'src> {
    pub direction: Spanned<Direction>,
    pub name: Ident<'src>,
    pub body: Body<'src>,
}

/// Which way a message travels, as written in `message[in]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    In,
    Out,
    Bi,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Body<'src> {
    pub fields: Vec<Field<'src>>,
    pub items: Vec<Item<'src>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field<'src> {
    pub name: Ident<'src>,
    pub ty: Type<'src>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Type<'src> {
    pub kind: TypeKind<'src>,
    /// The `[...]` written right after the type; they constrain this type
    /// only, not the types nested in it.
    pub constraints: Vec<Constraint<'src>>,
    /// Covers the constraints too.
    pub span: Span,
}

/// `name` or `name=value`, as in `i32[min=0, max=10]` or `f32[infinite]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Constraint<'src> {
    pub name: Ident<'src>,
    pub value: Option<Spanned<Number>>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Number {
    Int(i128),
    Float(f64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeKind<'src> {
    Prim(Prim),
    Array(Box<Type<'src>>, Spanned<u32>),
    Vec(Box<Type<'src>>),
    Set(Box<Type<'src>>),
    Map(Box<Type<'src>>, Box<Type<'src>>),
    /// Only allowed as a field's own type, around a non-collection.
    Option(Box<Type<'src>>),
    Named(Path<'src>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    Uuid,
    String,
    Bytes,
}

impl<'src> Path<'src> {
    pub fn name(&self) -> &Ident<'src> {
        self.segments
            .last()
            .expect("a path has one segment or more")
    }

    pub fn is_qualified(&self) -> bool {
        self.segments.len() > 1
    }
}

impl<'src> Item<'src> {
    pub fn name(&self) -> &Ident<'src> {
        match self {
            Item::Enum(item) => &item.name,
            Item::Struct(item) => &item.name,
            Item::Message(item) => &item.name,
        }
    }

    pub fn keyword(&self) -> &'static str {
        match self {
            Item::Enum(_) => "enum",
            Item::Struct(_) => "struct",
            Item::Message(_) => "message",
        }
    }

    pub fn body(&self) -> Option<&Body<'src>> {
        match self {
            Item::Struct(item) => Some(&item.body),
            Item::Message(item) => Some(&item.body),
            Item::Enum(_) => None,
        }
    }
}

impl Direction {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "in" => Direction::In,
            "out" => Direction::Out,
            "bi" => Direction::Bi,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Direction::In => "in",
            Direction::Out => "out",
            Direction::Bi => "bi",
        }
    }
}

impl Prim {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "i8" => Prim::I8,
            "i16" => Prim::I16,
            "i32" => Prim::I32,
            "i64" => Prim::I64,
            "u8" => Prim::U8,
            "u16" => Prim::U16,
            "u32" => Prim::U32,
            "u64" => Prim::U64,
            "f32" => Prim::F32,
            "f64" => Prim::F64,
            "bool" => Prim::Bool,
            "timestamp" => Prim::Timestamp,
            "interval" => Prim::Interval,
            "entity" => Prim::Entity,
            "uentity" => Prim::UEntity,
            "uuid" => Prim::Uuid,
            "string" => Prim::String,
            "bytes" => Prim::Bytes,
            _ => return None,
        })
    }

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
            Prim::Uuid => "uuid",
            Prim::String => "string",
            Prim::Bytes => "bytes",
        }
    }

    pub fn is_int(self) -> bool {
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

    pub fn is_unsigned_int(self) -> bool {
        matches!(self, Prim::U8 | Prim::U16 | Prim::U32 | Prim::U64)
    }
}

impl fmt::Display for Path<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, (segment, _)) in self.segments.iter().enumerate() {
            if index > 0 {
                f.write_str(".")?;
            }
            f.write_str(segment)?;
        }
        Ok(())
    }
}

impl fmt::Display for Type<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)?;
        if !self.constraints.is_empty() {
            f.write_str("[")?;
            for (index, constraint) in self.constraints.iter().enumerate() {
                if index > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{constraint}")?;
            }
            f.write_str("]")?;
        }
        Ok(())
    }
}

impl fmt::Display for Constraint<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name.0)?;
        if let Some((value, _)) = &self.value {
            write!(f, "={value}")?;
        }
        Ok(())
    }
}

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Number::Int(value) => write!(f, "{value}"),
            Number::Float(value) => write!(f, "{value:?}"),
        }
    }
}

impl fmt::Display for TypeKind<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeKind::Prim(prim) => f.write_str(prim.name()),
            TypeKind::Array(item, (len, _)) => write!(f, "array<{item}, {len}>"),
            TypeKind::Vec(item) => write!(f, "vec<{item}>"),
            TypeKind::Set(item) => write!(f, "set<{item}>"),
            TypeKind::Map(key, value) => write!(f, "map<{key}, {value}>"),
            TypeKind::Option(item) => write!(f, "option<{item}>"),
            TypeKind::Named(path) => write!(f, "{path}"),
        }
    }
}

impl fmt::Display for Prim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_PRIMS: [Prim; 18] = [
        Prim::I8,
        Prim::I16,
        Prim::I32,
        Prim::I64,
        Prim::U8,
        Prim::U16,
        Prim::U32,
        Prim::U64,
        Prim::F32,
        Prim::F64,
        Prim::Bool,
        Prim::Timestamp,
        Prim::Interval,
        Prim::Entity,
        Prim::UEntity,
        Prim::Uuid,
        Prim::String,
        Prim::Bytes,
    ];

    fn ty(kind: TypeKind<'_>) -> Type<'_> {
        Type {
            kind,
            constraints: Vec::new(),
            span: 0..0,
        }
    }

    fn path<'src>(segments: &[&'src str]) -> Path<'src> {
        Path {
            segments: segments.iter().map(|name| (*name, 0..0)).collect(),
            span: 0..0,
        }
    }

    #[test]
    fn prim_names_round_trip() {
        for prim in ALL_PRIMS {
            assert_eq!(Prim::from_name(prim.name()), Some(prim));
        }
    }

    #[test]
    fn containers_and_user_types_are_not_prims() {
        for name in [
            "vec", "set", "map", "array", "option", "Position", "u128", "int",
        ] {
            assert_eq!(Prim::from_name(name), None);
        }
    }

    #[test]
    fn only_unsigned_ints_can_back_an_enum() {
        let unsigned: Vec<_> = ALL_PRIMS
            .into_iter()
            .filter(|prim| prim.is_unsigned_int())
            .collect();
        assert_eq!(unsigned, [Prim::U8, Prim::U16, Prim::U32, Prim::U64]);
        let ints = ALL_PRIMS.into_iter().filter(|prim| prim.is_int()).count();
        assert_eq!(ints, 8);
    }

    #[test]
    fn types_print_as_written() {
        let nested = ty(TypeKind::Map(
            Box::new(ty(TypeKind::Prim(Prim::U8))),
            Box::new(ty(TypeKind::Vec(Box::new(ty(TypeKind::Named(path(&[
                "foo", "bar", "Position",
            ]))))))),
        ));
        assert_eq!(nested.to_string(), "map<u8, vec<foo.bar.Position>>");

        let array = ty(TypeKind::Array(
            Box::new(ty(TypeKind::Prim(Prim::I32))),
            (4, 0..0),
        ));
        assert_eq!(array.to_string(), "array<i32, 4>");

        let set = ty(TypeKind::Set(Box::new(ty(TypeKind::Prim(Prim::String)))));
        assert_eq!(set.to_string(), "set<string>");

        let option = ty(TypeKind::Option(Box::new(ty(TypeKind::Prim(Prim::U8)))));
        assert_eq!(option.to_string(), "option<u8>");

        let constraint = |name, value: Option<Number>| Constraint {
            name: (name, 0..0),
            value: value.map(|value| (value, 0..0)),
            span: 0..0,
        };
        let mut item = ty(TypeKind::Prim(Prim::F32));
        item.constraints = vec![
            constraint("min", Some(Number::Float(1.0))),
            constraint("infinite", None),
        ];
        let mut vec = ty(TypeKind::Vec(Box::new(item)));
        vec.constraints = vec![constraint("max_length", Some(Number::Int(4)))];
        assert_eq!(vec.to_string(), "vec<f32[min=1.0, infinite]>[max_length=4]");
    }

    #[test]
    fn paths_expose_their_last_segment() {
        let qualified = path(&["foo", "bar", "Position"]);
        assert!(qualified.is_qualified());
        assert_eq!(qualified.name().0, "Position");

        let local = path(&["Velocity"]);
        assert!(!local.is_qualified());
        assert_eq!(local.name().0, "Velocity");
        assert_eq!(local.to_string(), "Velocity");
    }

    #[test]
    fn items_report_their_name_and_keyword() {
        let item = Item::Message(Message {
            direction: (Direction::In, 8..10),
            name: ("MyRequest", 12..21),
            body: Body::default(),
        });
        assert_eq!(item.name(), &("MyRequest", 12..21));
        assert_eq!(item.keyword(), "message");
        assert_eq!(item.body(), Some(&Body::default()));

        let item = Item::Enum(Enum {
            name: ("Color", 5..10),
            repr: None,
            variants: Vec::new(),
        });
        assert_eq!(item.keyword(), "enum");
        assert_eq!(item.body(), None);
    }

    #[test]
    fn direction_names_round_trip() {
        for direction in [Direction::In, Direction::Out, Direction::Bi] {
            assert_eq!(Direction::from_name(direction.name()), Some(direction));
        }
        assert_eq!(Direction::from_name("inout"), None);
    }
}
