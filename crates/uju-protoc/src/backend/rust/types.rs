//! How schema types map to Rust: owned types, cells, and views, and
//! the names generated code uses.

use super::*;

impl Generator<'_> {
    /// `name`, declared in `id`'s module, as seen from `module`.
    pub(super) fn path(&self, id: TypeId, module: &[String], name: &str) -> String {
        let target = &self.modules[id.0];
        let common = target
            .iter()
            .zip(module)
            .take_while(|(left, right)| left == right)
            .count();
        let up = module.len() - common;
        let mut path = match (up, common == target.len()) {
            (0, true) => String::new(),
            // `self::` keeps a module from being read as a crate.
            (0, false) => "self::".to_string(),
            _ => "super::".repeat(up),
        };
        for segment in &target[common..] {
            path.push_str(segment);
            path.push_str("::");
        }
        path.push_str(name);
        path
    }

    pub(super) fn view_path(&self, id: TypeId, module: &[String]) -> String {
        self.path(id, module, &format!("{}View", self.names[id.0]))
    }

    pub(super) fn is_fixed_def(&self, id: TypeId) -> bool {
        match &self.schema.def(id).kind {
            DefKind::Enum(_) => true,
            DefKind::Type(item) => self.wire.size(&item.target).fixed().is_some(),
            DefKind::Union(_) => false,
            DefKind::Struct(_) | DefKind::Message(_) | DefKind::Hub(_) | DefKind::Component(_) => {
                self.wire.layout(id).size.fixed().is_some()
            }
            DefKind::Audience(_) | DefKind::Tag(_) => unreachable!("not a type"),
        }
    }

    /// The `type`s `ty` names in turn before its scalar, outermost first, as
    /// seen from `module`; empty when `ty` names none.
    pub(super) fn newtypes(&self, ty: &Type, module: &[String]) -> Vec<String> {
        let mut paths = Vec::new();
        let mut ty = ty;
        while let TypeKind::Named(id) = ty.kind
            && let DefKind::Type(item) = &self.schema.def(id).kind
        {
            paths.push(self.path(id, module, &self.names[id.0]));
            ty = &item.target;
        }
        paths
    }

    pub(super) fn fixed_size(&self, ty: &Type) -> u32 {
        self.wire.size(ty).fixed().expect("a fixed-size type")
    }

    pub(super) fn field_owned(&self, field: &Field, module: &[String]) -> String {
        let ty = self.owned(&field.ty, module);
        if field.optional {
            format!("{OPTION}<{ty}>")
        } else {
            ty
        }
    }

    /// The owned Rust type of `ty`.
    pub(super) fn owned(&self, ty: &Type, module: &[String]) -> String {
        if let Some(int) = IntInfo::of(ty) {
            return int.rust.to_string();
        }
        match &ty.kind {
            TypeKind::F32 => "f32".to_string(),
            TypeKind::F64 => "f64".to_string(),
            TypeKind::Bool => "bool".to_string(),
            TypeKind::Timestamp => SYSTEM_TIME.to_string(),
            TypeKind::Duration => DURATION.to_string(),
            TypeKind::Entity => "__w::Entity".to_string(),
            TypeKind::String => STRING.to_string(),
            TypeKind::Bytes => match ty.constraints.fixed_length() {
                Some(len) => format!("[u8; {len}]"),
                None => format!("{VEC}<u8>"),
            },
            TypeKind::Array(item, len) => format!("[{}; {len}]", self.owned(item, module)),
            TypeKind::Vec(item) => format!("{VEC}<{}>", self.owned(item, module)),
            TypeKind::Set(item) => {
                format!("::std::collections::HashSet<{}>", self.owned(item, module))
            }
            TypeKind::Map(key, value) => format!(
                "::std::collections::HashMap<{}, {}>",
                self.owned(key, module),
                self.owned(value, module)
            ),
            TypeKind::Named(id) => self.path(*id, module, &self.names[id.0]),
            _ => unreachable!("integers are handled above"),
        }
    }

    /// The cell a fixed-size `ty` is read through.
    pub(super) fn cell(&self, ty: &Type, module: &[String]) -> String {
        if let Some(int) = IntInfo::of(ty) {
            return int.cell();
        }
        match &ty.kind {
            TypeKind::F32 => "__w::F32".to_string(),
            TypeKind::F64 => "__w::F64".to_string(),
            TypeKind::Bool => "__w::Bool".to_string(),
            TypeKind::Timestamp => "__w::TimestampCell".to_string(),
            TypeKind::Duration => "__w::DurationCell".to_string(),
            TypeKind::Entity => "__w::EntityCell".to_string(),
            TypeKind::String => format!("__w::FixedStr<{}>", self.fixed_size(ty)),
            TypeKind::Bytes => format!("[u8; {}]", self.fixed_size(ty)),
            TypeKind::Array(item, len) => format!("[{}; {len}]", self.cell(item, module)),
            TypeKind::Named(id) => match self.schema.def(*id).kind {
                DefKind::Enum(_) => format!(
                    "__w::EnumCell<{}>",
                    self.path(*id, module, &self.names[id.0])
                ),
                DefKind::Type(_) => {
                    format!("__w::Typed<{}>", self.path(*id, module, &self.names[id.0]))
                }
                DefKind::Union(_) => unreachable!("a union is never fixed-size"),
                DefKind::Struct(_) | DefKind::Message(_) | DefKind::Hub(_) => {
                    self.view_path(*id, module)
                }
                DefKind::Audience(_) | DefKind::Component(_) | DefKind::Tag(_) => {
                    unreachable!("not a type")
                }
            },
            _ => unreachable!("only fixed-size types have cells"),
        }
    }

    /// What reading `ty` in place gives, borrowing for `lt`.
    pub(super) fn view_ty(&self, ty: &Type, module: &[String], lt: &str) -> String {
        if self.wire.size(ty).fixed().is_none() {
            return self.var_view(ty, module, lt);
        }
        if let Some(int) = IntInfo::of(ty) {
            return int.rust.to_string();
        }
        match &ty.kind {
            TypeKind::String => format!("&{lt} str"),
            TypeKind::Bytes => format!("&{lt} [u8; {}]", self.fixed_size(ty)),
            TypeKind::Array(..) => format!("&{lt} {}", self.cell(ty, module)),
            TypeKind::Named(id) => match self.schema.def(*id).kind {
                DefKind::Enum(_) | DefKind::Type(_) => self.path(*id, module, &self.names[id.0]),
                _ => format!("&{lt} {}", self.view_path(*id, module)),
            },
            _ => self.owned(ty, module),
        }
    }

    /// The view of a variable-size `ty`.
    pub(super) fn var_view(&self, ty: &Type, module: &[String], lt: &str) -> String {
        match &ty.kind {
            TypeKind::String => format!("&{lt} str"),
            TypeKind::Bytes => format!("&{lt} [u8]"),
            TypeKind::Vec(item) | TypeKind::Set(item) => {
                match self.wire.sequence(ty).expect("a sequence") {
                    Sequence::Packed { .. } => {
                        format!("__w::Packed<{lt}, {}>", self.cell(item, module))
                    }
                    Sequence::Counted { count } => format!(
                        "__w::Counted<{lt}, {}, {}>",
                        self.cell(item, module),
                        count.bytes()
                    ),
                    Sequence::Prefixed { count, prefix } => format!(
                        "__w::Prefixed<{lt}, {}, {}, {}>",
                        self.view_ty(item, module, lt),
                        count.bytes(),
                        prefix.bytes()
                    ),
                }
            }
            TypeKind::Map(key, value) => {
                let entry = || {
                    format!(
                        "__w::Entry<{}, {}>",
                        self.cell(key, module),
                        self.cell(value, module)
                    )
                };
                match self.wire.sequence(ty).expect("a sequence") {
                    Sequence::Packed { .. } => format!("__w::Packed<{lt}, {}>", entry()),
                    Sequence::Counted { count } => {
                        format!("__w::Counted<{lt}, {}, {}>", entry(), count.bytes())
                    }
                    Sequence::Prefixed { count, prefix } => format!(
                        "__w::PrefixedMap<{lt}, {}, {}, {}, {}>",
                        self.cell(key, module),
                        self.view_ty(value, module, lt),
                        count.bytes(),
                        prefix.bytes()
                    ),
                }
            }
            TypeKind::Named(id) => match self.schema.def(*id).kind {
                DefKind::Union(_) => self.union_view_ty(*id, module, lt),
                _ => format!("{}<{lt}>", self.view_path(*id, module)),
            },
            _ => unreachable!("only strings, bytes, collections, structs, and unions vary in size"),
        }
    }

    /// Converts `e`, a value read in place as `ty`, to the owned type.
    pub(super) fn owned_from(&self, ty: &Type, e: &str) -> String {
        match &ty.kind {
            TypeKind::String => format!("{STRING}::from({e})"),
            TypeKind::Bytes => match ty.constraints.fixed_length() {
                Some(_) => format!("*{e}"),
                None => format!("{e}.to_vec()"),
            },
            TypeKind::Array(item, _) => {
                if is_raw_byte(item) || IntInfo::of(item).is_some_and(|int| int.rust == "i8") {
                    format!("*{e}")
                } else {
                    let item = self.owned_from(item, "__w::Cell::get(c)");
                    format!("{e}.each_ref().map(|c| {item})")
                }
            }
            TypeKind::Vec(item) | TypeKind::Set(item) => {
                if matches!(ty.kind, TypeKind::Vec(_))
                    && is_raw_byte(item)
                    && matches!(self.wire.sequence(ty), Some(Sequence::Packed { .. }))
                {
                    return format!("{e}.as_slice().to_vec()");
                }
                match self.owned_from(item, "v").as_str() {
                    "v" => format!("{e}.iter().collect()"),
                    item => format!("{e}.iter().map(|v| {item}).collect()"),
                }
            }
            TypeKind::Map(key, value) => {
                let key = self.owned_from(key, "k");
                let value = self.owned_from(value, "v");
                if key == "k" && value == "v" {
                    format!("{e}.iter().collect()")
                } else {
                    format!("{e}.iter().map(|(k, v)| ({key}, {value})).collect()")
                }
            }
            TypeKind::Named(id) => match self.schema.def(*id).kind {
                DefKind::Enum(_) | DefKind::Type(_) => e.to_string(),
                _ => format!("{e}.to_owned()"),
            },
            _ => e.to_string(),
        }
    }
}

/// An integer's Rust type, bounds, and wire representation.
pub(super) struct IntInfo {
    pub(super) rust: &'static str,
    pub(super) lo: i128,
    pub(super) hi: i128,
    pub(super) min: i128,
    pub(super) max: i128,
    pub(super) repr: Int,
}

impl IntInfo {
    pub(super) fn of(ty: &Type) -> Option<IntInfo> {
        let (rust, min, max) = match ty.kind {
            TypeKind::I8 => ("i8", i8::MIN as i128, i8::MAX as i128),
            TypeKind::I16 => ("i16", i16::MIN as i128, i16::MAX as i128),
            TypeKind::I32 => ("i32", i32::MIN as i128, i32::MAX as i128),
            TypeKind::I64 => ("i64", i64::MIN as i128, i64::MAX as i128),
            TypeKind::U8 => ("u8", 0, u8::MAX as i128),
            TypeKind::U16 => ("u16", 0, u16::MAX as i128),
            TypeKind::U32 => ("u32", 0, u32::MAX as i128),
            TypeKind::U64 => ("u64", 0, u64::MAX as i128),
            _ => return None,
        };
        let bound = |bound: Option<Number>, default| match bound {
            Some(Number::Int(value)) => value,
            _ => default,
        };
        Some(IntInfo {
            rust,
            lo: bound(ty.constraints.min, min),
            hi: bound(ty.constraints.max, max),
            min,
            max,
            repr: Int::of(ty)?,
        })
    }

    pub(super) fn constrained(&self) -> bool {
        self.lo > self.min || self.hi < self.max
    }

    pub(super) fn span(&self) -> u128 {
        (self.hi - self.lo) as u128
    }

    /// `value` as a literal of the declared type.
    pub(super) fn lit(&self, value: i128) -> String {
        if value == self.min {
            format!("{}::MIN", self.rust)
        } else if value == self.max {
            format!("{}::MAX", self.rust)
        } else {
            value.to_string()
        }
    }

    pub(super) fn cell(&self) -> String {
        match self.repr {
            Int::Native(_) => match self.rust {
                "u8" | "i8" => self.rust.to_string(),
                rust => format!("__w::{}", rust.to_uppercase()),
            },
            // A negative const argument needs braces.
            Int::Biased { min, width } if min < 0 => {
                format!("__w::Biased<{}, {}, {{ {min} }}>", self.rust, stored(width))
            }
            Int::Biased { min, width } => {
                format!("__w::Biased<{}, {}, {min}>", self.rust, stored(width))
            }
        }
    }
}

/// The cell holding a biased integer of `width`.
pub(super) fn stored(width: Width) -> &'static str {
    match width {
        Width::U8 => "u8",
        Width::U16 => "__w::U16",
        Width::U32 => "__w::U32",
        Width::U64 => "__w::U64",
    }
}

pub(super) fn unsigned(width: Width) -> &'static str {
    match width {
        Width::U8 => "u8",
        Width::U16 => "u16",
        Width::U32 => "u32",
        Width::U64 => "u64",
    }
}

pub(super) fn width_max(width: Width) -> u128 {
    match width {
        Width::U8 => u8::MAX as u128,
        Width::U16 => u16::MAX as u128,
        Width::U32 => u32::MAX as u128,
        Width::U64 => u64::MAX as u128,
    }
}

/// A `u8` stored as itself, so a run of them is plain bytes.
pub(super) fn is_raw_byte(ty: &Type) -> bool {
    IntInfo::of(ty).is_some_and(|int| {
        int.rust == "u8" && !int.constrained() && matches!(int.repr, Int::Native(_))
    })
}

/// The `nan, infinite, min, max` arguments of a float check, or `None` when
/// every value is allowed.
pub(super) fn float_args(ty: &Type) -> Option<String> {
    let rust = match ty.kind {
        TypeKind::F32 => "f32",
        TypeKind::F64 => "f64",
        _ => return None,
    };
    let c = &ty.constraints;
    let bound = |bound: Option<Number>, default: &str| match bound {
        Some(Number::Float(value)) => format!("{value:?}"),
        Some(Number::Int(value)) => format!("{value}.0"),
        None => format!("{rust}::{default}"),
    };
    if c.nan && c.infinite && c.min.is_none() && c.max.is_none() {
        return None;
    }
    Some(format!(
        "{}, {}, {}, {}",
        c.nan,
        c.infinite,
        bound(c.min, "NEG_INFINITY"),
        bound(c.max, "INFINITY")
    ))
}

/// The `min, max` arguments of a length check, or `None` without
/// `min_length` or `max_length`.
pub(super) fn length_args(c: &Constraints) -> Option<String> {
    if c.min_length.is_none() && c.max_length.is_none() {
        return None;
    }
    let max = c
        .max_length
        .map_or_else(|| "__w::MAX_SIZE".to_string(), |max| max.to_string());
    Some(format!("{}, {max}", c.min_length.unwrap_or(0)))
}

pub(super) fn set_bit(bit: u32) -> String {
    format!("bits[{}] |= 0x{:02x};", bit / 8, 1u8 << (bit % 8))
}

/// `e.map(|v| inner)`, or `e` when `inner` is `v` itself.
pub(super) fn optional(e: &str, inner: &str) -> String {
    if inner == "v" {
        e.to_string()
    } else {
        format!("{e}.map(|v| {inner})")
    }
}

/// `name` as a Rust identifier: raw for keywords, with a trailing `_` for
/// the few that cannot be raw.
pub(super) fn ident(name: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "do",
        "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl", "in",
        "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
        "return", "static", "struct", "trait", "true", "try", "type", "typeof", "unsafe",
        "unsized", "use", "virtual", "where", "while", "yield",
    ];
    if matches!(name, "self" | "Self" | "super" | "crate" | "_") {
        format!("{name}_")
    } else if KEYWORDS.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}

/// `name` in snake case as a Rust identifier, for a module: `MyResponse`
/// becomes `my_response`, and `Type` becomes `r#type`.
pub(super) fn snake(name: &str) -> String {
    ident(&case::snake(name))
}

/// A field's accessor on a view, kept clear of the view's own methods.
pub(super) fn accessor(name: &str) -> String {
    if VIEW_METHODS.contains(&name) {
        format!("{name}_")
    } else {
        ident(name)
    }
}
