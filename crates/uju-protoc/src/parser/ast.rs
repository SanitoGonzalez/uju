use std::fmt;

pub use crate::ir::Role;
use crate::lexer::{Rfc3339, Span, Spanned};

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
    Const(Const<'src>),
    Union(Union<'src>),
    Type(TypeDecl<'src>),
    Hub(Hub<'src>),
}

/// `@name` or `@name(args)`, written above a declaration. Each declaration
/// struct holds its own in `attrs`, in source order.
#[derive(Debug, Clone, PartialEq)]
pub struct Attribute<'src> {
    pub name: Ident<'src>,
    /// Empty for a bare `@name`; `@name()` is a syntax error.
    pub args: Vec<AttributeArg<'src>>,
    /// From the `@` to the closing parenthesis, if any.
    pub span: Span,
}

/// An attribute's argument, as written.
#[derive(Debug, Clone, PartialEq)]
pub enum AttributeArg<'src> {
    Literal(Spanned<Literal>),
    /// `Faction` or `a.b.C`.
    Path(Path<'src>),
    /// `ops(InventoryOp)`. `args` is never empty.
    Call {
        path: Path<'src>,
        args: Vec<AttributeArg<'src>>,
        span: Span,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Enum<'src> {
    pub attrs: Vec<Attribute<'src>>,
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
    pub attrs: Vec<Attribute<'src>>,
    pub name: Ident<'src>,
    pub body: Body<'src>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message<'src> {
    pub attrs: Vec<Attribute<'src>>,
    pub endpoints: Endpoints<'src>,
    pub name: Ident<'src>,
    pub body: Body<'src>,
}

/// `(client | sim -> hub<Party>)`: the roles sending a message, then the
/// roles receiving it.
#[derive(Debug, Clone, PartialEq)]
pub struct Endpoints<'src> {
    pub from: Vec<Endpoint<'src>>,
    pub to: Vec<Endpoint<'src>>,
    /// `<->`: each side also sends to the other.
    pub both: bool,
    /// Covers the parentheses.
    pub span: Span,
}

/// `sim`, or `hub<Party>`, a hub kind's objects.
#[derive(Debug, Clone, PartialEq)]
pub struct Endpoint<'src> {
    pub role: Role,
    /// The kind in `hub<Kind>`. Only `hub` takes one.
    pub kind: Option<Path<'src>>,
    pub span: Span,
}

/// `hub(Key) Name { ... }`: a hub kind, whose objects are found by a `Key`
/// and hold the body as their state. Only at the top level of a namespace.
#[derive(Debug, Clone, PartialEq)]
pub struct Hub<'src> {
    pub attrs: Vec<Attribute<'src>>,
    pub key: Type<'src>,
    pub name: Ident<'src>,
    pub body: Body<'src>,
}

/// `union Name { A { x: u8 }, B(u8), C }`: a sum type whose variants
/// carry data.
#[derive(Debug, Clone, PartialEq)]
pub struct Union<'src> {
    pub attrs: Vec<Attribute<'src>>,
    pub name: Ident<'src>,
    pub variants: Vec<UnionVariant<'src>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnionVariant<'src> {
    pub name: Ident<'src>,
    pub data: VariantData<'src>,
}

/// What a union variant carries, in one of Rust's three forms.
#[derive(Debug, Clone, PartialEq)]
pub enum VariantData<'src> {
    /// `C`: nothing.
    Unit,
    /// `B(u8)`: a single value.
    Type(Type<'src>),
    /// `A { x: u8 }`: fields, laid out as an object. `span` covers the
    /// braces.
    Fields {
        fields: Vec<Field<'src>>,
        span: Span,
    },
}

/// `type Name = Target`: a named type, laid out as its target.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeDecl<'src> {
    pub attrs: Vec<Attribute<'src>>,
    pub name: Ident<'src>,
    pub target: Type<'src>,
}

/// `const Name: Type = value`, at the top level or in a struct or message.
#[derive(Debug, Clone, PartialEq)]
pub struct Const<'src> {
    pub attrs: Vec<Attribute<'src>>,
    pub name: Ident<'src>,
    pub ty: Type<'src>,
    pub value: Spanned<Literal>,
}

/// A const's value, as written.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int(i128),
    Float(f64),
    Bool(bool),
    String(String),
    /// `30s`: a count of `unit`s, unchecked against `duration`'s range.
    Duration(i128, DurationUnit),
    /// `2026-11-07T12:00:00Z`: microseconds since the Unix epoch.
    Timestamp(u64),
}

/// The unit written right after a duration literal's count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationUnit {
    /// `us`: a microsecond.
    Us,
    /// `ms`: a millisecond.
    Ms,
    /// `s`: a second.
    S,
    /// `m`: a minute.
    M,
    /// `h`: an hour.
    H,
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
    Duration,
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
            Item::Const(item) => &item.name,
            Item::Union(item) => &item.name,
            Item::Type(item) => &item.name,
            Item::Hub(item) => &item.name,
        }
    }

    pub fn attrs(&self) -> &[Attribute<'src>] {
        match self {
            Item::Enum(item) => &item.attrs,
            Item::Struct(item) => &item.attrs,
            Item::Message(item) => &item.attrs,
            Item::Const(item) => &item.attrs,
            Item::Union(item) => &item.attrs,
            Item::Type(item) => &item.attrs,
            Item::Hub(item) => &item.attrs,
        }
    }

    pub fn attrs_mut(&mut self) -> &mut Vec<Attribute<'src>> {
        match self {
            Item::Enum(item) => &mut item.attrs,
            Item::Struct(item) => &mut item.attrs,
            Item::Message(item) => &mut item.attrs,
            Item::Const(item) => &mut item.attrs,
            Item::Union(item) => &mut item.attrs,
            Item::Type(item) => &mut item.attrs,
            Item::Hub(item) => &mut item.attrs,
        }
    }

    pub fn keyword(&self) -> &'static str {
        match self {
            Item::Enum(_) => "enum",
            Item::Struct(_) => "struct",
            Item::Message(_) => "message",
            Item::Const(_) => "const",
            Item::Union(_) => "union",
            Item::Type(_) => "type",
            Item::Hub(_) => "hub",
        }
    }

    pub fn body(&self) -> Option<&Body<'src>> {
        match self {
            Item::Struct(item) => Some(&item.body),
            Item::Message(item) => Some(&item.body),
            Item::Hub(item) => Some(&item.body),
            Item::Enum(_) | Item::Const(_) | Item::Union(_) | Item::Type(_) => None,
        }
    }
}

impl AttributeArg<'_> {
    pub fn span(&self) -> &Span {
        match self {
            AttributeArg::Literal((_, span)) => span,
            AttributeArg::Path(path) => &path.span,
            AttributeArg::Call { span, .. } => span,
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
            "duration" => Prim::Duration,
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
            Prim::Duration => "duration",
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

impl DurationUnit {
    /// Every unit, smallest first.
    pub const ALL: [DurationUnit; 5] = [
        DurationUnit::Us,
        DurationUnit::Ms,
        DurationUnit::S,
        DurationUnit::M,
        DurationUnit::H,
    ];

    pub fn from_suffix(suffix: &str) -> Option<Self> {
        Some(match suffix {
            "us" => DurationUnit::Us,
            "ms" => DurationUnit::Ms,
            "s" => DurationUnit::S,
            "m" => DurationUnit::M,
            "h" => DurationUnit::H,
            _ => return None,
        })
    }

    pub fn suffix(self) -> &'static str {
        match self {
            DurationUnit::Us => "us",
            DurationUnit::Ms => "ms",
            DurationUnit::S => "s",
            DurationUnit::M => "m",
            DurationUnit::H => "h",
        }
    }

    /// How many microseconds one of this unit lasts.
    pub fn micros(self) -> u64 {
        match self {
            DurationUnit::Us => 1,
            DurationUnit::Ms => 1_000,
            DurationUnit::S => 1_000_000,
            DurationUnit::M => 60_000_000,
            DurationUnit::H => 3_600_000_000,
        }
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

impl fmt::Display for Attribute<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.name.0)?;
        write_args(f, &self.args)
    }
}

impl fmt::Display for AttributeArg<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AttributeArg::Literal((literal, _)) => write!(f, "{literal}"),
            AttributeArg::Path(path) => write!(f, "{path}"),
            AttributeArg::Call { path, args, .. } => {
                write!(f, "{path}")?;
                write_args(f, args)
            }
        }
    }
}

/// `(a, b)`, or nothing for no arguments.
fn write_args(f: &mut fmt::Formatter<'_>, args: &[AttributeArg<'_>]) -> fmt::Result {
    if args.is_empty() {
        return Ok(());
    }
    f.write_str("(")?;
    for (index, arg) in args.iter().enumerate() {
        if index > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{arg}")?;
    }
    f.write_str(")")
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

impl fmt::Display for Literal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Literal::Int(value) => write!(f, "{value}"),
            Literal::Float(value) => write!(f, "{value:?}"),
            Literal::Bool(value) => write!(f, "{value}"),
            Literal::String(value) => write!(f, "{value:?}"),
            Literal::Duration(count, unit) => write!(f, "{count}{unit}"),
            Literal::Timestamp(micros) => write!(f, "{}", Rfc3339(*micros)),
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

impl fmt::Display for DurationUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.suffix())
    }
}

impl fmt::Display for Endpoint<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.role.name())?;
        if let Some(kind) = &self.kind {
            write!(f, "<{kind}>")?;
        }
        Ok(())
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
        Prim::Duration,
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
            attrs: Vec::new(),
            endpoints: Endpoints {
                from: vec![Endpoint {
                    role: Role::Client,
                    kind: None,
                    span: 8..14,
                }],
                to: vec![Endpoint {
                    role: Role::Sim,
                    kind: None,
                    span: 18..21,
                }],
                both: false,
                span: 7..22,
            },
            name: ("MyRequest", 23..32),
            body: Body::default(),
        });
        assert_eq!(item.name(), &("MyRequest", 23..32));
        assert_eq!(item.keyword(), "message");
        assert_eq!(item.body(), Some(&Body::default()));

        let item = Item::Enum(Enum {
            attrs: Vec::new(),
            name: ("Color", 5..10),
            repr: None,
            variants: Vec::new(),
        });
        assert_eq!(item.keyword(), "enum");
        assert_eq!(item.body(), None);

        let item = Item::Union(Union {
            attrs: Vec::new(),
            name: ("Op", 6..8),
            variants: Vec::new(),
        });
        assert_eq!(item.keyword(), "union");
        assert_eq!(item.body(), None, "a union holds no nested declarations");
    }

    #[test]
    fn duration_units_round_trip_and_scale() {
        for unit in DurationUnit::ALL {
            assert_eq!(DurationUnit::from_suffix(unit.suffix()), Some(unit));
            assert_eq!(unit.to_string(), unit.suffix());
        }
        assert_eq!(
            DurationUnit::ALL.map(DurationUnit::micros),
            [1, 1_000, 1_000_000, 60_000_000, 3_600_000_000]
        );
        assert_eq!(DurationUnit::from_suffix("ns"), None);
        assert_eq!(DurationUnit::from_suffix("d"), None);
    }

    #[test]
    fn duration_and_timestamp_literals_print_as_written() {
        assert_eq!(Literal::Duration(30, DurationUnit::S).to_string(), "30s");
        assert_eq!(
            Literal::Duration(1000, DurationUnit::Ms).to_string(),
            "1000ms"
        );
        assert_eq!(Literal::Duration(-5, DurationUnit::M).to_string(), "-5m");
        assert_eq!(Literal::Timestamp(0).to_string(), "1970-01-01T00:00:00Z");
        assert_eq!(
            Literal::Timestamp(1_000_250_000).to_string(),
            "1970-01-01T00:16:40.250Z"
        );
        assert_eq!(
            Literal::Timestamp(1_000_250_125).to_string(),
            "1970-01-01T00:16:40.250125Z"
        );
    }

    #[test]
    fn role_names_round_trip() {
        for role in Role::ALL {
            assert_eq!(Role::from_name(role.name()), Some(role));
        }
        assert_eq!(Role::from_name("coordinator"), None);
        assert_eq!(Role::from_name("Sim"), None);
    }

    #[test]
    fn endpoints_print_as_written() {
        let hub = Endpoint {
            role: Role::Hub,
            kind: Some(path(&["a", "Party"])),
            span: 0..0,
        };
        assert_eq!(hub.to_string(), "hub<a.Party>");
        let sim = Endpoint {
            role: Role::Sim,
            kind: None,
            span: 0..0,
        };
        assert_eq!(sim.to_string(), "sim");
    }
}
