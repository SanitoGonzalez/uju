use std::collections::HashSet;

use crate::diagnostic::{Diagnostic, SourceId};
use crate::ir::wire::{self, Wire};
use crate::ir::*;
use crate::lexer::Span;
use crate::parser::ast;
use crate::resolve::{ConstSite, Resolver, TypeSite};

/// Lower parsed schemas into the IR, resolving every name and validating
/// every declaration. `schemas` are compiled together and indexed by
/// [`SourceId`]; the IR is only produced when no errors were found.
pub fn lower(schemas: &[ast::Schema<'_>]) -> Result<Schema, Vec<Diagnostic>> {
    let (resolver, diagnostics) = Resolver::new(schemas);
    let mut lowerer = Lowerer {
        resolver: &resolver,
        diagnostics,
        fixed_checks: Vec::new(),
    };

    let defs: Vec<Def> = resolver
        .types
        .iter()
        .enumerate()
        .map(|(index, site)| lowerer.lower_def(TypeId(index), site))
        .collect();
    let consts: Vec<Const> = resolver
        .consts
        .iter()
        .map(|site| lowerer.lower_const(site))
        .collect();

    let schema = Schema {
        namespaces: (0..resolver.namespace_len())
            .map(|index| Namespace {
                name: resolver
                    .namespace_name(NamespaceId(index))
                    .iter()
                    .map(|segment| segment.to_string())
                    .collect(),
            })
            .collect(),
        defs,
        consts,
    };

    // Whether a struct is fixed-size depends on its fields, so these wait
    // until every declaration is lowered.
    lowerer.check_fixed(&schema);
    // Only meaningful once every type has resolved; unresolved fields carry
    // placeholder types that would produce nonsense cycles.
    if lowerer.diagnostics.is_empty() {
        lowerer.check_cycles(&schema.defs);
    }
    // Sizes are only finite once cycles are ruled out.
    if lowerer.diagnostics.is_empty() {
        lowerer.check_wire(&schema);
    }
    if !lowerer.diagnostics.is_empty() {
        return Err(lowerer.diagnostics);
    }

    Ok(schema)
}

struct Lowerer<'a, 'src> {
    resolver: &'a Resolver<'a, 'src>,
    diagnostics: Vec<Diagnostic>,
    fixed_checks: Vec<FixedCheck>,
}

/// A type that must be fixed-size, checked once every declaration is
/// lowered.
struct FixedCheck {
    source: SourceId,
    span: Span,
    /// The type as written.
    written: String,
    ty: Type,
    /// What the type is, as in "a set item".
    role: &'static str,
}

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    White,
    Grey,
    Black,
}

impl<'a, 'src> Lowerer<'a, 'src> {
    fn error(&mut self, source: SourceId, span: &Span, message: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::new(source, span.clone(), message));
    }

    fn lower_def(&mut self, id: TypeId, site: &TypeSite<'a, 'src>) -> Def {
        self.check_attrs(site.source, site.item.attrs());
        let kind = match site.item {
            ast::Item::Enum(item) => DefKind::Enum(self.lower_enum(site.source, item)),
            ast::Item::Struct(item) => DefKind::Struct(self.lower_struct(site.source, id, item)),
            ast::Item::Message(item) => DefKind::Message(self.lower_message(site.source, id, item)),
            ast::Item::Union(item) => DefKind::Union(self.lower_union(site.source, id, item)),
            ast::Item::Type(item) => DefKind::Type(self.lower_type_def(site.source, id, item)),
            ast::Item::Const(_) => unreachable!("consts are not type sites"),
        };
        Def {
            namespace: site.namespace,
            parent: site.parent,
            name: site.item.name().0.to_string(),
            kind,
        }
    }

    /// Check a declaration's attributes, each given once. No attribute is
    /// defined yet, so every one is unknown.
    fn check_attrs(&mut self, source: SourceId, attrs: &[ast::Attribute<'src>]) {
        let mut seen = HashSet::new();
        for attr in attrs {
            let (name, span) = &attr.name;
            if !seen.insert(*name) {
                self.error(source, span, format!("duplicate attribute `{name}`"));
                continue;
            }
            self.error(source, span, format!("unknown attribute `{name}`"));
        }
    }

    /// Check a const's type, which must be a number, `bool`, `string`,
    /// `duration`, or `timestamp` without constraints, and its value against
    /// the type.
    fn lower_const(&mut self, site: &ConstSite<'a, 'src>) -> Const {
        let source = site.source;
        let item = site.item;
        self.check_attrs(source, &item.attrs);
        let lowered = |ty, value| Const {
            namespace: site.namespace,
            parent: site.parent,
            name: item.name.0.to_string(),
            ty,
            value,
        };
        if let Some(constraint) = item.ty.constraints.first() {
            self.error(
                source,
                &constraint.span,
                "a const's type takes no constraints",
            );
        }
        let ty = match &item.ty.kind {
            ast::TypeKind::Prim(prim)
                if prim.is_int()
                    || matches!(
                        prim,
                        ast::Prim::F32
                            | ast::Prim::F64
                            | ast::Prim::Bool
                            | ast::Prim::String
                            | ast::Prim::Duration
                            | ast::Prim::Timestamp
                    ) =>
            {
                prim_type(*prim)
            }
            kind => {
                self.error(
                    source,
                    &item.ty.span,
                    format!(
                        "a const cannot be `{kind}`; it takes an integer, a float, `bool`, `string`, \
                         `duration`, or `timestamp`"
                    ),
                );
                return lowered(TypeKind::Bool, ConstValue::Bool(false));
            }
        };

        let (literal, span) = &item.value;
        let kind_name = &item.ty.kind;
        let (value, expected) = match (constraint_class(&ty), literal) {
            (Class::Int(min, max), ast::Literal::Int(value)) => {
                if !(min..=max).contains(value) {
                    self.error(
                        source,
                        span,
                        format!("`{value}` is out of range for `{kind_name}`"),
                    );
                }
                (Some(ConstValue::Int(*value)), "")
            }
            (Class::Int(..), _) => (None, "an integer"),
            // Integers widen, as in a float's `min` and `max`.
            (Class::Float(limit), ast::Literal::Int(_) | ast::Literal::Float(_)) => {
                let value = match *literal {
                    ast::Literal::Int(value) => value as f64,
                    ast::Literal::Float(value) => value,
                    _ => unreachable!("matched a number"),
                };
                if value.abs() > limit {
                    self.error(
                        source,
                        span,
                        format!("`{value:?}` is out of range for `{kind_name}`"),
                    );
                }
                (Some(ConstValue::Float(value)), "")
            }
            (Class::Float(_), _) => (None, "a number"),
            _ => match (&ty, literal) {
                (TypeKind::Bool, ast::Literal::Bool(value)) => (Some(ConstValue::Bool(*value)), ""),
                (TypeKind::Bool, _) => (None, "`true` or `false`"),
                (TypeKind::Duration, ast::Literal::Duration(count, unit)) => {
                    // Overflow is out of range too, so it can't wrap into it.
                    let micros = count
                        .checked_mul(unit.micros() as i128)
                        .filter(|micros| (0..=MAX_DURATION_MICROS as i128).contains(micros));
                    if micros.is_none() {
                        self.error(
                            source,
                            span,
                            format!("`{literal}` is out of range for `{kind_name}`"),
                        );
                    }
                    let micros = micros.unwrap_or(0) as u64;
                    (Some(ConstValue::Duration(micros)), "")
                }
                (TypeKind::Duration, _) => (None, "a duration such as `30s`"),
                (TypeKind::Timestamp, ast::Literal::Timestamp(micros)) => {
                    (Some(ConstValue::Timestamp(*micros)), "")
                }
                (TypeKind::Timestamp, _) => (None, "a timestamp such as `2026-01-01T00:00:00Z`"),
                (_, ast::Literal::String(value)) => (Some(ConstValue::String(value.clone())), ""),
                _ => (None, "a string"),
            },
        };
        let value = value.unwrap_or_else(|| {
            let found = match literal {
                ast::Literal::Int(_) => "an integer",
                ast::Literal::Float(_) => "a float",
                ast::Literal::Bool(_) => "a bool",
                ast::Literal::String(_) => "a string",
                ast::Literal::Duration(..) => "a duration",
                ast::Literal::Timestamp(_) => "a timestamp",
            };
            self.error(
                source,
                span,
                format!("`{kind_name}` takes {expected}, found {found}"),
            );
            ConstValue::Bool(false)
        });
        lowered(ty, value)
    }

    fn lower_enum(&mut self, source: SourceId, item: &ast::Enum<'src>) -> Enum {
        let repr = match &item.repr {
            None => EnumRepr::U32,
            Some((ast::Prim::U8, _)) => EnumRepr::U8,
            Some((ast::Prim::U16, _)) => EnumRepr::U16,
            Some((ast::Prim::U32, _)) => EnumRepr::U32,
            Some((ast::Prim::U64, _)) => EnumRepr::U64,
            Some((prim, span)) => {
                self.error(
                    source,
                    span,
                    format!(
                        "enum `{}` cannot be backed by `{prim}`; only unsigned integers are allowed",
                        item.name.0
                    ),
                );
                EnumRepr::U32
            }
        };

        if item.variants.is_empty() {
            self.error(
                source,
                &item.name.1,
                format!("enum `{}` needs at least one variant", item.name.0),
            );
        }

        let mut variants: Vec<Variant> = Vec::with_capacity(item.variants.len());
        let mut next = Some(0u64);
        for variant in &item.variants {
            if variants.iter().any(|seen| seen.name == variant.name.0) {
                self.error(
                    source,
                    &variant.name.1,
                    format!("duplicate variant `{}`", variant.name.0),
                );
                continue;
            }
            let value = match &variant.value {
                Some((value, span)) => {
                    if *value < 0 || *value > repr.max() as i128 {
                        self.error(
                            source,
                            span,
                            format!("`{value}` is out of range for `{repr}`"),
                        );
                        continue;
                    }
                    *value as u64
                }
                None => match next {
                    Some(value) => value,
                    None => {
                        self.error(
                            source,
                            &variant.name.1,
                            format!("variant value exceeds the maximum of `{repr}`"),
                        );
                        continue;
                    }
                },
            };
            if let Some(previous) = variants.iter().find(|seen| seen.value == value) {
                let span = variant
                    .value
                    .as_ref()
                    .map_or(&variant.name.1, |(_, span)| span);
                self.error(
                    source,
                    &span.clone(),
                    format!(
                        "`{}` and `{}` share the value {value}",
                        previous.name, variant.name.0
                    ),
                );
            }
            next = value.checked_add(1).filter(|next| *next <= repr.max());
            variants.push(Variant {
                name: variant.name.0.to_string(),
                value,
            });
        }

        Enum { repr, variants }
    }

    fn lower_fields(
        &mut self,
        source: SourceId,
        scope: TypeId,
        fields: &[ast::Field<'src>],
    ) -> Vec<Field> {
        let mut seen = HashSet::new();
        fields
            .iter()
            .map(|field| {
                if !seen.insert(field.name.0) {
                    self.error(
                        source,
                        &field.name.1,
                        format!("duplicate field `{}`", field.name.0),
                    );
                }
                // `option` is unwrapped here, the one place it is allowed.
                let (ty, optional) = match &field.ty.kind {
                    ast::TypeKind::Option(item) => {
                        if matches!(
                            item.kind,
                            ast::TypeKind::Array(..)
                                | ast::TypeKind::Vec(_)
                                | ast::TypeKind::Set(_)
                                | ast::TypeKind::Map(..)
                        ) {
                            self.error(
                                source,
                                &item.span,
                                format!("`{item}` is a collection; collections cannot be optional"),
                            );
                        }
                        if let Some(constraint) = field.ty.constraints.first() {
                            self.error(
                                source,
                                &constraint.span,
                                format!(
                                    "an `option` takes no constraints; constrain its value instead, as in `option<{}[{constraint}]>`",
                                    item.kind
                                ),
                            );
                        }
                        (&**item, true)
                    }
                    _ => (&field.ty, false),
                };
                Field {
                    name: field.name.0.to_string(),
                    ty: self.lower_type(source, Some(scope), ty),
                    optional,
                }
            })
            .collect()
    }

    fn lower_struct(&mut self, source: SourceId, id: TypeId, item: &ast::Struct<'src>) -> Struct {
        Struct {
            fields: self.lower_fields(source, id, &item.body.fields),
        }
    }

    fn lower_union(&mut self, source: SourceId, id: TypeId, item: &ast::Union<'src>) -> Union {
        let name = item.name.0;
        if item.variants.is_empty() {
            self.error(
                source,
                &item.name.1,
                format!("union `{name}` needs at least one variant"),
            );
        }
        if item.variants.len() > wire::MAX_VARIANTS {
            self.error(
                source,
                &item.name.1,
                format!(
                    "union `{name}` has {} variants; a union holds at most {}",
                    item.variants.len(),
                    wire::MAX_VARIANTS
                ),
            );
        }
        let mut seen = HashSet::new();
        let variants = item
            .variants
            .iter()
            .map(|variant| {
                let (variant_name, span) = &variant.name;
                if !seen.insert(*variant_name) {
                    self.error(source, span, format!("duplicate variant `{variant_name}`"));
                }
                let data = match &variant.data {
                    ast::VariantData::Unit => VariantData::Unit,
                    ast::VariantData::Type(ty) => {
                        VariantData::Type(self.lower_type(source, Some(id), ty))
                    }
                    ast::VariantData::Fields { fields, span } => {
                        // One spelling for a variant without data.
                        if fields.is_empty() {
                            self.error(
                                source,
                                span,
                                format!(
                                    "variant `{variant_name}` has no fields; write it as `{variant_name}`"
                                ),
                            );
                        }
                        VariantData::Fields(self.lower_fields(source, id, fields))
                    }
                };
                UnionVariant {
                    name: variant_name.to_string(),
                    data,
                }
            })
            .collect();
        Union { variants }
    }

    /// A `type` stands for a scalar: what is read by value, so its name can
    /// wrap the value wherever it is read.
    fn lower_type_def(
        &mut self,
        source: SourceId,
        id: TypeId,
        item: &ast::TypeDecl<'src>,
    ) -> TypeDef {
        let written = match &item.target.kind {
            ast::TypeKind::Option(inner) => {
                self.error(
                    source,
                    &item.target.span,
                    "a `type` cannot be optional; make the field holding it optional instead",
                );
                &**inner
            }
            _ => &item.target,
        };
        let target = self.lower_type(source, Some(id), written);
        let scalar = match &target.kind {
            TypeKind::Array(..)
            | TypeKind::Vec(_)
            | TypeKind::Set(_)
            | TypeKind::Map(..)
            | TypeKind::String
            | TypeKind::Bytes => false,
            TypeKind::Named(target) => matches!(
                self.resolver.types[target.0].item,
                ast::Item::Enum(_) | ast::Item::Type(_)
            ),
            _ => true,
        };
        if !scalar {
            self.error(
                source,
                &written.span,
                format!(
                    "`{written}` is not a scalar; a `type` stands for an integer, a float, `bool`, `timestamp`, `duration`, `entity`, `uentity`, `uuid`, an enum, or another `type`"
                ),
            );
        }
        TypeDef { target }
    }

    fn lower_message(
        &mut self,
        source: SourceId,
        id: TypeId,
        item: &ast::Message<'src>,
    ) -> Message {
        let direction = match item.direction.0 {
            ast::Direction::In => Direction::In,
            ast::Direction::Out => Direction::Out,
            ast::Direction::Bi => Direction::Bi,
        };
        Message {
            direction,
            fields: self.lower_fields(source, id, &item.body.fields),
        }
    }

    fn lower_type(
        &mut self,
        source: SourceId,
        scope: Option<TypeId>,
        ty: &ast::Type<'src>,
    ) -> Type {
        let kind = match &ty.kind {
            ast::TypeKind::Prim(prim) => prim_type(*prim),
            ast::TypeKind::Array(item, (len, _)) => {
                let item_ty = self.lower_type(source, scope, item);
                self.require_fixed(source, item, &item_ty, "an array item");
                TypeKind::Array(Box::new(item_ty), *len)
            }
            ast::TypeKind::Vec(item) => {
                TypeKind::Vec(Box::new(self.lower_type(source, scope, item)))
            }
            ast::TypeKind::Set(item) => {
                let item_ty = self.lower_type(source, scope, item);
                self.check_key(source, scope, item, &item_ty, "a set item");
                TypeKind::Set(Box::new(item_ty))
            }
            ast::TypeKind::Map(key, value) => {
                let key_ty = self.lower_type(source, scope, key);
                self.check_key(source, scope, key, &key_ty, "a map key");
                TypeKind::Map(
                    Box::new(key_ty),
                    Box::new(self.lower_type(source, scope, value)),
                )
            }
            ast::TypeKind::Option(item) => {
                self.error(
                    source,
                    &ty.span,
                    format!("`{ty}` is nested; `option` is only allowed as a field's own type"),
                );
                return self.lower_type(source, scope, item);
            }
            ast::TypeKind::Named(path) => match self.resolver.resolve(source, scope, path) {
                Ok(id) => {
                    if matches!(self.resolver.types[id.0].item, ast::Item::Message(_)) {
                        self.error(
                            source,
                            &ty.span,
                            format!(
                                "`{path}` is a message; messages are standalone and cannot be used as a type"
                            ),
                        );
                        TypeKind::Bool
                    } else {
                        TypeKind::Named(id)
                    }
                }
                Err(diagnostic) => {
                    self.diagnostics.push(diagnostic);
                    TypeKind::Bool
                }
            },
        };
        let constraints = self.lower_constraints(source, ty, &kind);
        Type { kind, constraints }
    }

    fn require_fixed(
        &mut self,
        source: SourceId,
        written: &ast::Type<'src>,
        ty: &Type,
        role: &'static str,
    ) {
        self.fixed_checks.push(FixedCheck {
            source,
            span: written.span.clone(),
            written: written.to_string(),
            ty: ty.clone(),
            role,
        });
    }

    fn check_fixed(&mut self, schema: &Schema) {
        for check in std::mem::take(&mut self.fixed_checks) {
            if !schema.is_fixed(&check.ty) {
                self.error(
                    check.source,
                    &check.span,
                    format!(
                        "`{}` is variable-size; {} must be fixed-size",
                        check.written, check.role
                    ),
                );
            }
        }
    }

    /// Report what the wire format cannot hold, such as a value larger than
    /// any message.
    fn check_wire(&mut self, schema: &Schema) {
        let Err(errors) = Wire::new(schema) else {
            return;
        };
        for error in errors {
            let site = &self.resolver.types[error.def.0];
            let span = match (site.item, error.variant, error.field) {
                (ast::Item::Union(item), Some(variant), field) => {
                    let variant = &item.variants[variant];
                    match (&variant.data, field) {
                        (ast::VariantData::Type(ty), _) => ty.span.clone(),
                        (ast::VariantData::Fields { fields, .. }, Some(field)) => {
                            fields[field].ty.span.clone()
                        }
                        _ => variant.name.1.clone(),
                    }
                }
                (ast::Item::Type(item), _, _) => item.target.span.clone(),
                (item, _, Some(field)) => match item.body() {
                    Some(body) => body.fields[field].ty.span.clone(),
                    None => item.name().1.clone(),
                },
                (item, _, None) => item.name().1.clone(),
            };
            self.diagnostics
                .push(Diagnostic::new(site.source, span, error.message));
        }
    }

    /// Set items and map keys must be fixed-size and comparable, which rules
    /// out floats anywhere inline: `NaN != NaN` and `-0.0 == 0.0` break
    /// lookup, and many targets (Rust among them) cannot hash or order
    /// floats at all.
    fn check_key(
        &mut self,
        source: SourceId,
        scope: Option<TypeId>,
        key: &ast::Type<'src>,
        key_ty: &Type,
        role: &'static str,
    ) {
        self.require_fixed(source, key, key_ty, role);
        if let Some(path) = self.holds_float(source, scope, key, &mut Vec::new()) {
            let through = if path.is_empty() {
                String::new()
            } else {
                format!(" through field `{}`", path.join("."))
            };
            self.error(
                source,
                &key.span,
                format!("`{key}` holds a float{through}; {role} cannot hold a float"),
            );
        }
    }

    /// The field path to a float that a value of `ty` stores inline,
    /// directly (an empty path) or through a struct, an array, or a `type`.
    /// `visiting` guards against recursive types, which are reported
    /// separately.
    fn holds_float(
        &self,
        source: SourceId,
        scope: Option<TypeId>,
        ty: &ast::Type<'src>,
        visiting: &mut Vec<TypeId>,
    ) -> Option<Vec<&'src str>> {
        match &ty.kind {
            ast::TypeKind::Prim(ast::Prim::F32 | ast::Prim::F64) => Some(Vec::new()),
            ast::TypeKind::Array(item, _) | ast::TypeKind::Option(item) => {
                self.holds_float(source, scope, item, visiting)
            }
            ast::TypeKind::Named(path) => {
                let id = self.resolver.resolve(source, scope, path).ok()?;
                let site = &self.resolver.types[id.0];
                if visiting.contains(&id) {
                    return None;
                }
                let item = match site.item {
                    ast::Item::Struct(item) => item,
                    ast::Item::Type(item) => {
                        visiting.push(id);
                        let path = self.holds_float(site.source, Some(id), &item.target, visiting);
                        visiting.pop();
                        return path;
                    }
                    _ => return None,
                };
                visiting.push(id);
                item.body.fields.iter().find_map(|field| {
                    let mut path = self.holds_float(site.source, Some(id), &field.ty, visiting)?;
                    path.insert(0, field.name.0);
                    Some(path)
                })
            }
            ast::TypeKind::Prim(_)
            | ast::TypeKind::Vec(_)
            | ast::TypeKind::Set(_)
            | ast::TypeKind::Map(..) => None,
        }
    }

    /// Check the constraints written on `ty` against the type they constrain.
    fn lower_constraints(
        &mut self,
        source: SourceId,
        ty: &ast::Type<'src>,
        kind: &TypeKind,
    ) -> Constraints {
        let mut out = Constraints::default();
        if ty.constraints.is_empty() {
            return out;
        }

        let class = constraint_class(kind);
        let allowed: &[&str] = match class {
            Class::Int(..) => &["min", "max"],
            Class::Float(_) => &["min", "max", "infinite", "nan"],
            Class::Length => &["min_length", "max_length"],
            Class::None => &[],
        };
        let kind_name = &ty.kind;

        let mut seen: Vec<&str> = Vec::new();
        let mut max_span = None;
        let mut max_length_span = None;
        for constraint in &ty.constraints {
            let (name, name_span) = &constraint.name;
            if allowed.is_empty() {
                self.error(
                    source,
                    name_span,
                    format!("`{kind_name}` takes no constraints"),
                );
                continue;
            }
            if !allowed.contains(name) {
                let expected = allowed
                    .iter()
                    .map(|name| format!("`{name}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                self.error(
                    source,
                    name_span,
                    format!(
                        "`{name}` is not a constraint of `{kind_name}`; expected one of {expected}"
                    ),
                );
                continue;
            }
            if seen.contains(name) {
                self.error(source, name_span, format!("duplicate constraint `{name}`"));
                continue;
            }
            seen.push(name);

            let Some((value, value_span)) = &constraint.value else {
                if *name == "infinite" {
                    out.infinite = true;
                } else if *name == "nan" {
                    out.nan = true;
                } else {
                    self.error(
                        source,
                        &constraint.span,
                        format!("`{name}` needs a value, as in `{name}=0`"),
                    );
                }
                continue;
            };
            match *name {
                "infinite" | "nan" => {
                    self.error(source, value_span, format!("`{name}` takes no value"));
                }
                "min" | "max" => {
                    let Some(number) = self.bound(source, value_span, *value, &class, kind_name)
                    else {
                        continue;
                    };
                    if *name == "min" {
                        out.min = Some(number);
                    } else {
                        out.max = Some(number);
                        max_span = Some(value_span);
                    }
                }
                _ => {
                    let length = match *value {
                        ast::Number::Int(length) => u64::try_from(length).ok(),
                        ast::Number::Float(_) => None,
                    };
                    let Some(length) = length else {
                        self.error(
                            source,
                            value_span,
                            format!("`{name}` must be a non-negative integer"),
                        );
                        continue;
                    };
                    if length > wire::MAX_SIZE as u64 {
                        self.error(
                            source,
                            value_span,
                            format!(
                                "`{name}` must be at most {}, the size of a message",
                                wire::MAX_SIZE
                            ),
                        );
                        continue;
                    }
                    if *name == "min_length" {
                        out.min_length = Some(length);
                    } else {
                        out.max_length = Some(length);
                        max_length_span = Some(value_span);
                    }
                }
            }
        }

        let inverted = match (out.min, out.max) {
            (Some(Number::Int(min)), Some(Number::Int(max))) => min > max,
            (Some(Number::Float(min)), Some(Number::Float(max))) => min > max,
            _ => false,
        };
        if let (true, Some(span)) = (inverted, max_span) {
            self.error(source, span, "`max` is less than `min`");
        }
        if let (Some(min), Some(max), Some(span)) =
            (out.min_length, out.max_length, max_length_span)
            && min > max
        {
            self.error(source, span, "`max_length` is less than `min_length`");
        }

        out
    }

    /// Check a `min`/`max` value against the number it bounds.
    fn bound(
        &mut self,
        source: SourceId,
        span: &Span,
        value: ast::Number,
        class: &Class,
        kind_name: &ast::TypeKind<'src>,
    ) -> Option<Number> {
        match (class, value) {
            (Class::Int(min, max), ast::Number::Int(value)) => {
                if (*min..=*max).contains(&value) {
                    Some(Number::Int(value))
                } else {
                    self.error(
                        source,
                        span,
                        format!("`{value}` is out of range for `{kind_name}`"),
                    );
                    None
                }
            }
            (Class::Int(..), ast::Number::Float(value)) => {
                self.error(
                    source,
                    span,
                    format!("`{value:?}` is not an integer; `{kind_name}` takes integer bounds"),
                );
                None
            }
            (Class::Float(limit), value) => {
                let value = match value {
                    ast::Number::Int(value) => value as f64,
                    ast::Number::Float(value) => value,
                };
                if value.abs() <= *limit {
                    Some(Number::Float(value))
                } else {
                    self.error(
                        source,
                        span,
                        format!("`{value:?}` is out of range for `{kind_name}`"),
                    );
                    None
                }
            }
            (Class::Length | Class::None, _) => unreachable!("only numbers take `min`/`max`"),
        }
    }

    /// Report recursive types, through any field: inline, optional, or
    /// inside a collection. Recursion would let a message nest as deep as
    /// its size allows, and every decoder would have to bound its stack.
    fn check_cycles(&mut self, defs: &[Def]) {
        let mut marks = vec![Mark::White; defs.len()];
        for index in 0..defs.len() {
            if marks[index] == Mark::White {
                self.visit(defs, &mut marks, index);
            }
        }
    }

    fn visit(&mut self, defs: &[Def], marks: &mut [Mark], index: usize) {
        marks[index] = Mark::Grey;
        let mut targets = Vec::new();
        match &defs[index].kind {
            DefKind::Enum(_) => {}
            DefKind::Struct(Struct { fields }) | DefKind::Message(Message { fields, .. }) => {
                for field in fields {
                    refs(&field.ty, &mut targets);
                }
            }
            DefKind::Union(item) => {
                for variant in &item.variants {
                    match &variant.data {
                        VariantData::Unit => {}
                        VariantData::Type(ty) => refs(ty, &mut targets),
                        VariantData::Fields(fields) => {
                            for field in fields {
                                refs(&field.ty, &mut targets);
                            }
                        }
                    }
                }
            }
            DefKind::Type(item) => refs(&item.target, &mut targets),
        }
        for target in targets {
            match marks[target.0] {
                Mark::Black => {}
                Mark::White => self.visit(defs, marks, target.0),
                Mark::Grey => {
                    let site = &self.resolver.types[target.0];
                    let name = site.item.name();
                    self.diagnostics.push(Diagnostic::new(
                        site.source,
                        name.1.clone(),
                        format!(
                            "`{}` refers to itself; recursive types are not allowed",
                            name.0
                        ),
                    ));
                }
            }
        }
        marks[index] = Mark::Black;
    }
}

/// The named types a value of `ty` refers to, directly or through a
/// collection.
fn refs(ty: &Type, out: &mut Vec<TypeId>) {
    match &ty.kind {
        TypeKind::Named(id) => out.push(*id),
        TypeKind::Array(item, _) | TypeKind::Vec(item) | TypeKind::Set(item) => refs(item, out),
        TypeKind::Map(key, value) => {
            refs(key, out);
            refs(value, out);
        }
        _ => {}
    }
}

/// The longest `duration` a const can take, in microseconds: 10000 Julian
/// years, or 100 centuries of 36525 days.
const MAX_DURATION_MICROS: u64 = 100 * 36_525 * 86_400 * 1_000_000;

/// Which constraints a type takes.
enum Class {
    /// Integers, with the inclusive range of the type.
    Int(i128, i128),
    /// Floats, with the largest finite magnitude of the type.
    Float(f64),
    /// Strings, bytes, and variable-size collections.
    Length,
    None,
}

fn constraint_class(kind: &TypeKind) -> Class {
    match kind {
        TypeKind::I8 => Class::Int(i8::MIN as i128, i8::MAX as i128),
        TypeKind::I16 => Class::Int(i16::MIN as i128, i16::MAX as i128),
        TypeKind::I32 => Class::Int(i32::MIN as i128, i32::MAX as i128),
        TypeKind::I64 => Class::Int(i64::MIN as i128, i64::MAX as i128),
        TypeKind::U8 => Class::Int(0, u8::MAX as i128),
        TypeKind::U16 => Class::Int(0, u16::MAX as i128),
        TypeKind::U32 => Class::Int(0, u32::MAX as i128),
        TypeKind::U64 => Class::Int(0, u64::MAX as i128),
        TypeKind::F32 => Class::Float(f32::MAX as f64),
        TypeKind::F64 => Class::Float(f64::MAX),
        TypeKind::String
        | TypeKind::Bytes
        | TypeKind::Vec(_)
        | TypeKind::Set(_)
        | TypeKind::Map(..) => Class::Length,
        _ => Class::None,
    }
}

fn prim_type(prim: ast::Prim) -> TypeKind {
    match prim {
        ast::Prim::I8 => TypeKind::I8,
        ast::Prim::I16 => TypeKind::I16,
        ast::Prim::I32 => TypeKind::I32,
        ast::Prim::I64 => TypeKind::I64,
        ast::Prim::U8 => TypeKind::U8,
        ast::Prim::U16 => TypeKind::U16,
        ast::Prim::U32 => TypeKind::U32,
        ast::Prim::U64 => TypeKind::U64,
        ast::Prim::F32 => TypeKind::F32,
        ast::Prim::F64 => TypeKind::F64,
        ast::Prim::Bool => TypeKind::Bool,
        ast::Prim::Timestamp => TypeKind::Timestamp,
        ast::Prim::Duration => TypeKind::Duration,
        ast::Prim::Entity => TypeKind::Entity,
        ast::Prim::UEntity => TypeKind::UEntity,
        ast::Prim::Uuid => TypeKind::Uuid,
        ast::Prim::String => TypeKind::String,
        ast::Prim::Bytes => TypeKind::Bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile;

    const EXAMPLE1: &str = include_str!("../../tests/data/example1.uju");
    const EXAMPLE2: &str = include_str!("../../tests/data/example2.uju");

    fn compiled(sources: &[&str]) -> Schema {
        compile(sources)
            .unwrap_or_else(|errors| panic!("unexpected errors: {errors:#?}"))
            .schema
    }

    fn messages(sources: &[&str]) -> Vec<String> {
        compile(sources)
            .expect_err("expected errors")
            .into_iter()
            .filter(|diagnostic| diagnostic.is_error())
            .map(|diagnostic| diagnostic.message)
            .collect()
    }

    fn def<'a>(schema: &'a Schema, name: &str) -> (TypeId, &'a Def) {
        let index = schema
            .defs
            .iter()
            .position(|def| def.name == name)
            .unwrap_or_else(|| panic!("no declaration named `{name}`"));
        (TypeId(index), &schema.defs[index])
    }

    #[test]
    fn examples_lower() {
        let schema = compiled(&[EXAMPLE1, EXAMPLE2]);
        assert_eq!(
            schema
                .namespaces
                .iter()
                .map(|namespace| namespace.name.join("."))
                .collect::<Vec<_>>(),
            ["foo.bar", "hello"]
        );

        let (_, color) = def(&schema, "Color");
        let DefKind::Enum(color) = &color.kind else {
            panic!("expected an enum");
        };
        assert_eq!(color.repr, EnumRepr::U8);
        assert_eq!(
            color
                .variants
                .iter()
                .map(|variant| (variant.name.as_str(), variant.value))
                .collect::<Vec<_>>(),
            [("Red", 0), ("Green", 1), ("Blue", 5)]
        );

        let (_, numbers) = def(&schema, "Numbers");
        let DefKind::Enum(numbers) = &numbers.kind else {
            panic!("expected an enum");
        };
        assert_eq!(numbers.repr, EnumRepr::U32, "the repr defaults to u32");
        assert_eq!(numbers.variants[2].value, 1_000_000_000);

        assert_eq!(
            constant(&schema, "Motd").value,
            ConstValue::String("Welcome!\n\u{1F44B}".into())
        );
        let (request, _) = def(&schema, "MyRequest");
        assert_eq!(constant(&schema, "MaxAddresses").parent, Some(request));
    }

    #[test]
    fn messages_carry_their_direction() {
        let schema = compiled(&[EXAMPLE1]);
        let message = |name: &str| {
            let (_, def) = def(&schema, name);
            let DefKind::Message(message) = &def.kind else {
                panic!("expected a message");
            };
            message.clone()
        };
        assert_eq!(message("Ping").direction, Direction::Bi);
        assert_eq!(message("MyRequest").direction, Direction::In);

        let (response_id, _) = def(&schema, "MyResponse");
        let (result_id, result) = def(&schema, "Result");
        assert_eq!(result.parent, Some(response_id));
        assert_eq!(schema.path(result_id), ["MyResponse", "Result"]);

        let response = message("MyResponse");
        assert_eq!(response.direction, Direction::Out);
        assert_eq!(response.fields[0].ty.kind, TypeKind::Named(result_id));
    }

    #[test]
    fn container_types_lower() {
        let schema = compiled(&[concat!(
            "namespace a\n",
            "struct Position { x: f32 }\n",
            "message[in] NonScalars {\n",
            "    v: vec<i32>,\n",
            "    ar: array<i32, 4>,\n",
            "    m2: map<u8, vec<Position>>,\n",
            "    o: option<i32>,\n",
            "    os: option<string>,\n",
            "}\n",
        )]);
        let (position_id, _) = def(&schema, "Position");
        let (_, non_scalars) = def(&schema, "NonScalars");
        let DefKind::Message(non_scalars) = &non_scalars.kind else {
            panic!("expected a message");
        };
        let field = |name: &str| {
            non_scalars
                .fields
                .iter()
                .find(|field| field.name == name)
                .unwrap_or_else(|| panic!("no field named `{name}`"))
        };

        let ty = |kind: TypeKind| Box::new(Type::from(kind));
        assert_eq!(field("ar").ty.kind, TypeKind::Array(ty(TypeKind::I32), 4));
        assert_eq!(
            field("m2").ty.kind,
            TypeKind::Map(
                ty(TypeKind::U8),
                ty(TypeKind::Vec(ty(TypeKind::Named(position_id))))
            )
        );
        assert_eq!(field("o").ty.kind, TypeKind::I32);
        assert!(field("o").optional);
        assert_eq!(field("os").ty.kind, TypeKind::String);
        assert!(field("os").optional);
        assert!(!field("v").optional);
    }

    #[test]
    fn imports_resolve_across_sources() {
        let schema = compiled(&[EXAMPLE1, EXAMPLE2]);
        let (color_id, color) = def(&schema, "Color");
        let (position_id, _) = def(&schema, "Position");
        assert_eq!(schema.namespace(color.namespace).name.join("."), "foo.bar");

        let (_, external) = def(&schema, "ExternalTypes");
        let DefKind::Message(external) = &external.kind else {
            panic!("expected a message");
        };
        assert_eq!(external.fields[0].ty.kind, TypeKind::Named(color_id));
        assert_eq!(external.fields[1].ty.kind, TypeKind::Named(position_id));
    }

    #[test]
    fn nested_declarations_shadow_outer_ones() {
        let src = "namespace a\nenum R { X }\nmessage[in] M {\n    enum R { Y }\n    r: R,\n}\n";
        let schema = compiled(&[src]);
        let (_, message) = def(&schema, "M");
        let DefKind::Message(message) = &message.kind else {
            panic!("expected a message");
        };
        let TypeKind::Named(id) = message.fields[0].ty.kind else {
            panic!("expected a named type");
        };
        assert!(schema.def(id).parent.is_some(), "the nested enum wins");

        // The nested declaration is reachable from outside by its path.
        compiled(&["namespace a\nmessage[in] M { enum R { Y } r: R, }\nstruct S { r: M.R }\n"]);
    }

    #[test]
    fn unknown_names_are_reported() {
        assert_eq!(
            messages(&["namespace a\nstruct S { p: Position }\n"]),
            ["cannot find `Position` in this scope"]
        );
        assert_eq!(
            messages(&["namespace a\nuse b\n"]),
            ["unknown namespace `b`"]
        );
        assert_eq!(
            messages(&[
                "namespace a\nstruct T {}\n",
                "namespace b\nuse a\nstruct S { t: a.X }\n",
            ]),
            ["no `X` in namespace `a`"]
        );
        assert_eq!(
            messages(&["namespace a\nenum E { X }\nstruct S { e: E.X }\n"]),
            ["`E` has no nested declaration `X`"]
        );
    }

    #[test]
    fn qualified_paths_need_a_use() {
        let a = "namespace a\nstruct T {}\n";
        let b = "namespace b\nstruct S { t: a.T }\n";
        assert_eq!(messages(&[a, b]), ["cannot find `a` in this scope"]);
    }

    #[test]
    fn unqualified_imports_can_be_ambiguous() {
        let a = "namespace a\nstruct T {}\n";
        let b = "namespace b\nstruct T {}\n";
        let c = "namespace c\nuse a\nuse b\nstruct S { t: T }\n";
        let messages = messages(&[a, b, c]);
        assert_eq!(
            messages,
            ["`T` is ambiguous; it is declared in namespaces `a` and `b`"]
        );
    }

    #[test]
    fn duplicate_declarations_are_reported() {
        assert_eq!(
            messages(&["namespace a\nstruct S {}\nenum S { A }\n"]),
            ["the name `S` is declared more than once in this scope"]
        );
        assert_eq!(
            messages(&["namespace a\nstruct S { x: i32, x: i32 }\n"]),
            ["duplicate field `x`"]
        );
        // The same name in two files sharing a namespace also collides.
        assert_eq!(
            messages(&["namespace a\nstruct S {}\n", "namespace a\nstruct S {}\n"]),
            ["the name `S` is declared more than once in this scope"]
        );
    }

    #[test]
    fn every_attribute_is_unknown_for_now() {
        let src = concat!(
            "namespace a\n",
            "@visibility(Faction)\n",
            "struct S {\n",
            "    @replicate(ops(Op))\n",
            "    enum E { A }\n",
            "    x: u8,\n",
            "}\n",
            "@large message[in] M {}\n",
            "@lock(Inventory, Gold) union U { A }\n",
            "@halo(false) type T = u8\n",
            "@timeout_ms(1000) const C: u8 = 1\n",
        );
        let errors = compile(&[src]).expect_err("expected errors");
        assert_eq!(
            errors
                .iter()
                .map(|error| (&src[error.span.clone()], error.message.as_str()))
                .collect::<Vec<_>>(),
            [
                ("visibility", "unknown attribute `visibility`"),
                ("replicate", "unknown attribute `replicate`"),
                ("large", "unknown attribute `large`"),
                ("lock", "unknown attribute `lock`"),
                ("halo", "unknown attribute `halo`"),
                ("timeout_ms", "unknown attribute `timeout_ms`"),
            ]
        );
    }

    #[test]
    fn an_attribute_is_given_once() {
        let src = "namespace a\n@a @b\n@a(1) struct S {}\n";
        let errors = compile(&[src]).expect_err("expected errors");
        assert_eq!(
            errors
                .iter()
                .map(|error| (error.span.start, error.message.as_str()))
                .collect::<Vec<_>>(),
            [
                (13, "unknown attribute `a`"),
                (16, "unknown attribute `b`"),
                (19, "duplicate attribute `a`"),
            ]
        );
        assert_eq!(&src[19..21], "a(", "the second occurrence");
    }

    #[test]
    fn enums_are_validated() {
        assert_eq!(
            messages(&["namespace a\nenum E: i32 { A }\n"]),
            ["enum `E` cannot be backed by `i32`; only unsigned integers are allowed"]
        );
        assert_eq!(
            messages(&["namespace a\nenum E: u8 { A = 256 }\n"]),
            ["`256` is out of range for `u8`"]
        );
        assert_eq!(
            messages(&["namespace a\nenum E: u8 { A = 255, B }\n"]),
            ["variant value exceeds the maximum of `u8`"]
        );
        assert_eq!(
            messages(&["namespace a\nenum E { A, B = 0 }\n"]),
            ["`A` and `B` share the value 0"]
        );
        assert_eq!(
            messages(&["namespace a\nenum E { A, A }\n"]),
            ["duplicate variant `A`"]
        );
        assert_eq!(
            messages(&["namespace a\nenum E {}\n"]),
            ["enum `E` needs at least one variant"]
        );
    }

    fn constant<'a>(schema: &'a Schema, name: &str) -> &'a Const {
        schema
            .consts
            .iter()
            .find(|item| item.name == name)
            .unwrap_or_else(|| panic!("no const named `{name}`"))
    }

    #[test]
    fn consts_lower_with_their_type_and_scope() {
        let schema = compiled(&[concat!(
            "namespace a\n",
            "const Count: u8 = 255\n",
            "const Low: i64 = -9223372036854775808\n",
            "const Big: u64 = 18446744073709551615\n",
            "const Rate: f32 = 0.1\n",
            "const Whole: f64 = 2\n",
            "const On: bool = true\n",
            "const Greeting: string = \"héllo\"\n",
            "message[in] M {\n",
            "    struct Inner { const Deep: u8 = 1 }\n",
            "    const MaxLength: u16 = 16\n",
            "}\n",
        )]);
        let value = |name| {
            (
                constant(&schema, name).ty.clone(),
                constant(&schema, name).value.clone(),
            )
        };
        assert_eq!(value("Count"), (TypeKind::U8, ConstValue::Int(255)));
        assert_eq!(
            value("Low"),
            (TypeKind::I64, ConstValue::Int(i64::MIN as i128))
        );
        assert_eq!(
            value("Big"),
            (TypeKind::U64, ConstValue::Int(u64::MAX as i128))
        );
        assert_eq!(value("Rate"), (TypeKind::F32, ConstValue::Float(0.1)));
        assert_eq!(value("Whole"), (TypeKind::F64, ConstValue::Float(2.0)));
        assert_eq!(value("On"), (TypeKind::Bool, ConstValue::Bool(true)));
        assert_eq!(
            value("Greeting"),
            (TypeKind::String, ConstValue::String("héllo".into()))
        );

        let (message, _) = def(&schema, "M");
        let (inner, _) = def(&schema, "Inner");
        assert_eq!(constant(&schema, "Count").parent, None);
        assert_eq!(constant(&schema, "MaxLength").parent, Some(message));
        assert_eq!(constant(&schema, "Deep").parent, Some(inner));
        let id = |name| {
            ConstId(
                schema
                    .consts
                    .iter()
                    .position(|item| item.name == name)
                    .unwrap(),
            )
        };
        assert_eq!(schema.const_qualified(id("Count")), "a.Count");
        assert_eq!(schema.const_qualified(id("MaxLength")), "a.M.MaxLength");
        assert_eq!(schema.const_qualified(id("Deep")), "a.M.Inner.Deep");
    }

    #[test]
    fn const_types_and_values_are_checked() {
        let cases = [
            ("const A: u8 = 256", "`256` is out of range for `u8`"),
            ("const A: u32 = -1", "`-1` is out of range for `u32`"),
            (
                "const A: f32 = 1000000000000000000000000000000000000000.0",
                "`1e39` is out of range for `f32`",
            ),
            ("const A: u8 = 1.5", "`u8` takes an integer, found a float"),
            (
                "const A: i32 = \"1\"",
                "`i32` takes an integer, found a string",
            ),
            ("const A: f64 = true", "`f64` takes a number, found a bool"),
            (
                "const A: bool = 1",
                "`bool` takes `true` or `false`, found an integer",
            ),
            (
                "const A: string = 1",
                "`string` takes a string, found an integer",
            ),
            (
                "const A: u8[max=3] = 1",
                "a const's type takes no constraints",
            ),
            (
                "const A: bytes = \"x\"",
                "a const cannot be `bytes`; it takes an integer, a float, `bool`, `string`, `duration`, or `timestamp`",
            ),
            (
                "const A: uuid = 0",
                "a const cannot be `uuid`; it takes an integer, a float, `bool`, `string`, `duration`, or `timestamp`",
            ),
            (
                "enum E { X }\nconst A: E = 0",
                "a const cannot be `E`; it takes an integer, a float, `bool`, `string`, `duration`, or `timestamp`",
            ),
            (
                "const A: vec<u8> = 0",
                "a const cannot be `vec<u8>`; it takes an integer, a float, `bool`, `string`, `duration`, or `timestamp`",
            ),
        ];
        for (src, expected) in cases {
            assert_eq!(
                messages(&[&format!("namespace a\n{src}\n")]),
                [expected],
                "{src}"
            );
        }
    }

    #[test]
    fn duration_and_timestamp_consts_lower_to_micros() {
        let schema = compiled(&[concat!(
            "namespace a\n",
            "const Zero: duration = 0s\n",
            "const Micro: duration = 250us\n",
            "const Second: duration = 1000ms\n",
            "const Hour: duration = 2h\n",
            "const Longest: duration = 87660000h\n",
            "const Epoch: timestamp = 1970-01-01T00:00:00Z\n",
            "const Start: timestamp = 2026-11-07T12:00:00.250125Z\n",
        )]);
        let value = |name| {
            (
                constant(&schema, name).ty.clone(),
                constant(&schema, name).value.clone(),
            )
        };
        assert_eq!(value("Zero"), (TypeKind::Duration, ConstValue::Duration(0)));
        assert_eq!(
            value("Micro"),
            (TypeKind::Duration, ConstValue::Duration(250))
        );
        assert_eq!(
            value("Second"),
            (TypeKind::Duration, ConstValue::Duration(1_000_000))
        );
        assert_eq!(
            value("Hour"),
            (TypeKind::Duration, ConstValue::Duration(7_200_000_000))
        );
        assert_eq!(
            value("Longest"),
            (
                TypeKind::Duration,
                ConstValue::Duration(315_576_000_000_000_000)
            )
        );
        assert_eq!(
            value("Epoch"),
            (TypeKind::Timestamp, ConstValue::Timestamp(0))
        );
        assert_eq!(
            value("Start"),
            (
                TypeKind::Timestamp,
                ConstValue::Timestamp(1_794_052_800_250_125)
            )
        );
    }

    #[test]
    fn equal_spellings_lower_to_equal_values() {
        let schema = compiled(&[concat!(
            "namespace a\n",
            "const A: duration = 90m\n",
            "const B: duration = 5400s\n",
            "const C: duration = 5400000000us\n",
            "const D: timestamp = 2026-11-07T12:00:00Z\n",
            "const E: timestamp = 2026-11-07T12:00:00.000Z\n",
            "const F: timestamp = 2026-11-07T12:00:00.000000Z\n",
        )]);
        let value = |name| constant(&schema, name).value.clone();
        assert_eq!(value("A"), value("B"));
        assert_eq!(value("A"), value("C"));
        assert_eq!(value("D"), value("E"));
        assert_eq!(value("D"), value("F"));
    }

    #[test]
    fn duration_and_timestamp_consts_are_checked() {
        let cases = [
            (
                "const A: duration = -1s",
                "`-1s` is out of range for `duration`",
            ),
            (
                "const A: duration = 87660001h",
                "`87660001h` is out of range for `duration`",
            ),
            (
                "const A: duration = 315576000000000001us",
                "`315576000000000001us` is out of range for `duration`",
            ),
            // Past `i128` once scaled, so it must not wrap into range.
            (
                "const A: duration = 170141183460469231731687303715884105727h",
                "`170141183460469231731687303715884105727h` is out of range for `duration`",
            ),
            (
                "const A: duration = 1000",
                "`duration` takes a duration such as `30s`, found an integer",
            ),
            (
                "const A: duration = 1.5",
                "`duration` takes a duration such as `30s`, found a float",
            ),
            (
                "const A: duration = 2026-01-01T00:00:00Z",
                "`duration` takes a duration such as `30s`, found a timestamp",
            ),
            (
                "const A: timestamp = 0",
                "`timestamp` takes a timestamp such as `2026-01-01T00:00:00Z`, found an integer",
            ),
            (
                "const A: timestamp = \"2026-01-01T00:00:00Z\"",
                "`timestamp` takes a timestamp such as `2026-01-01T00:00:00Z`, found a string",
            ),
            (
                "const A: timestamp = 30s",
                "`timestamp` takes a timestamp such as `2026-01-01T00:00:00Z`, found a duration",
            ),
            (
                "const A: u64 = 30s",
                "`u64` takes an integer, found a duration",
            ),
            (
                "const A: i64 = 2026-01-01T00:00:00Z",
                "`i64` takes an integer, found a timestamp",
            ),
            (
                "const A: f64 = 30s",
                "`f64` takes a number, found a duration",
            ),
            (
                "const A: f32 = 2026-01-01T00:00:00Z",
                "`f32` takes a number, found a timestamp",
            ),
            (
                "const A: bool = 1s",
                "`bool` takes `true` or `false`, found a duration",
            ),
            (
                "const A: string = 2026-01-01T00:00:00Z",
                "`string` takes a string, found a timestamp",
            ),
            (
                "const A: duration[max=1] = 1s",
                "a const's type takes no constraints",
            ),
        ];
        for (src, expected) in cases {
            assert_eq!(
                messages(&[&format!("namespace a\n{src}\n")]),
                [expected],
                "{src}"
            );
        }
    }

    #[test]
    fn consts_share_scopes_with_types() {
        assert_eq!(
            messages(&["namespace a\nstruct A {}\nconst A: u8 = 1\n"]),
            ["the name `A` is declared more than once in this scope"]
        );
        assert_eq!(
            messages(&["namespace a\nmessage[in] M { const A: u8 = 1 enum A { X } }\n"]),
            ["the name `A` is declared more than once in this scope"]
        );
        // The same name in different scopes is fine.
        compiled(&["namespace a\nconst A: u8 = 1\nstruct S { const A: u8 = 2 }\n"]);

        assert_eq!(
            messages(&["namespace a\nconst A: u8 = 1\nstruct S { a: A }\n"]),
            ["`A` is a const, not a type"]
        );
        assert_eq!(
            messages(&["namespace a\nconst A: u8 = 1\nstruct S { a: A.B }\n"]),
            ["`A` is a const and has no nested declarations"]
        );
        assert_eq!(
            messages(&["namespace a\nstruct T { const C: u8 = 1 }\nstruct S { c: T.C }\n"]),
            ["`T.C` is a const, not a type"]
        );
        // The innermost name wins, even when it is a const.
        assert_eq!(
            messages(&["namespace a\nstruct R {}\nmessage[in] M { const R: u8 = 1 r: R }\n"]),
            ["`R` is a const, not a type"]
        );
    }

    #[test]
    fn messages_are_standalone() {
        assert_eq!(
            messages(&["namespace a\nmessage[in] M {}\nstruct S { m: M }\n"]),
            ["`M` is a message; messages are standalone and cannot be used as a type"]
        );
        assert_eq!(
            messages(&["namespace a\nmessage[in] M {}\nmessage[out] N { v: vec<M> }\n"]).len(),
            1
        );
    }

    #[test]
    fn structs_can_hold_variable_size_and_optional_fields() {
        compiled(&["namespace a\nstruct S { v: vec<i32>, s: string, x: option<i32> }\n"]);
    }

    #[test]
    fn fixed_size_depends_on_the_fields() {
        let prelude = concat!(
            "namespace a\n",
            "struct Var { s: string }\n",
            "struct Optional { x: option<u8> }\n",
            "struct Flags { a: option<bool> }\n",
            "struct Code { c: string[min_length=2, max_length=2] }\n",
        );
        let cases = [
            (
                "a: array<Var, 2>",
                "`Var` is variable-size; an array item must be fixed-size",
            ),
            (
                "s: set<Optional>",
                "`Optional` is variable-size; a set item must be fixed-size",
            ),
            (
                "m: map<string[max_length=2], u8>",
                "`string[max_length=2]` is variable-size; a map key must be fixed-size",
            ),
        ];
        for (field, expected) in cases {
            let src = format!("{prelude}message[in] M {{ {field} }}\n");
            assert_eq!(messages(&[&src]), [expected], "{field}");
        }

        // A fixed length makes strings fixed-size, and `option<bool>` stays
        // in the bitmap.
        compiled(&[&format!(
            "{prelude}message[in] M {{ a: array<Flags, 2>, s: set<Code>, m: map<bytes[min_length=4, max_length=4], u8> }}\n"
        )]);
    }

    #[test]
    fn lengths_fit_a_message() {
        assert_eq!(
            messages(&["namespace a\nmessage[in] M { s: string[max_length=65536] }\n"]),
            ["`max_length` must be at most 65535, the size of a message"]
        );
        compiled(&["namespace a\nmessage[in] M { s: vec<u8>[max_length=65535] }\n"]);
    }

    #[test]
    fn containers_need_fixed_size_items() {
        assert_eq!(
            messages(&["namespace a\nmessage[in] M { a: array<vec<i32>, 2> }\n"]),
            ["`vec<i32>` is variable-size; an array item must be fixed-size"]
        );
        assert_eq!(
            messages(&["namespace a\nmessage[in] M { s: set<string> }\n"]),
            ["`string` is variable-size; a set item must be fixed-size"]
        );
        assert_eq!(
            messages(&["namespace a\nmessage[in] M { m: map<string, i32> }\n"]),
            ["`string` is variable-size; a map key must be fixed-size"]
        );
        // Map values may be variable-size.
        compiled(&["namespace a\nmessage[in] M { m: map<i32, vec<i32>> }\n"]);
    }

    #[test]
    fn collections_cannot_be_optional() {
        for ty in ["vec<i32>", "set<i32>", "map<i32, i32>", "array<i32, 2>"] {
            assert_eq!(
                messages(&[&format!(
                    "namespace a\nmessage[in] M {{ o: option<{ty}> }}\n"
                )]),
                [format!(
                    "`{ty}` is a collection; collections cannot be optional"
                )]
            );
        }
        // Variable-size scalars and user types may be.
        compiled(&[concat!(
            "namespace a\n",
            "struct P { x: f32 }\n",
            "enum E { A }\n",
            "message[in] M { s: option<string>, b: option<bytes>, p: option<P>, e: option<E> }\n",
        )]);
    }

    #[test]
    fn floats_cannot_be_keys() {
        let prelude = concat!(
            "namespace a\n",
            "struct Position { x: i32, y: f32 }\n",
            "struct Outer { id: u8, p: array<Position, 2> }\n",
        );
        let cases = [
            (
                "s: set<f32>",
                "`f32` holds a float; a set item cannot hold a float",
            ),
            (
                "m: map<f64[infinite], i32>",
                "`f64[infinite]` holds a float; a map key cannot hold a float",
            ),
            (
                "m: map<array<f32, 2>, i32>",
                "`array<f32, 2>` holds a float; a map key cannot hold a float",
            ),
            (
                "s: set<Position>",
                "`Position` holds a float through field `y`; a set item cannot hold a float",
            ),
            (
                "s: set<Outer>",
                "`Outer` holds a float through field `p.y`; a set item cannot hold a float",
            ),
        ];
        for (field, expected) in cases {
            let src = format!("{prelude}message[in] M {{ {field} }}\n");
            assert_eq!(messages(&[&src]), [expected], "{field}");
        }

        // Floats are fine as map values, and float-free structs as keys.
        compiled(&[concat!(
            "namespace a\n",
            "struct Cell { x: i32, y: i32 }\n",
            "message[in] M { m: map<Cell, f32[nan]>, s: set<Cell> }\n",
        )]);
    }

    #[test]
    fn constraints_lower_onto_the_type_they_follow() {
        let schema = compiled(&[EXAMPLE1]);
        let (_, request) = def(&schema, "MyRequest");
        let DefKind::Message(request) = &request.kind else {
            panic!("expected a message");
        };
        let foo = &request.fields[0].ty.constraints;
        assert_eq!(foo.min, Some(Number::Int(-100)));
        assert_eq!(foo.max, Some(Number::Int(100)));
        let addresses = &request.fields[2].ty;
        assert_eq!(addresses.constraints.max_length, Some(14));
        let TypeKind::Vec(element) = &addresses.kind else {
            panic!("expected a vec");
        };
        assert_eq!(
            element.constraints.max_length,
            Some(16),
            "each item carries its own constraints, apart from the vec's"
        );

        let (_, numbers) = def(&schema, "StrangeNumbers");
        let DefKind::Struct(numbers) = &numbers.kind else {
            panic!("expected a struct");
        };
        assert!(numbers.fields[0].ty.constraints.infinite);
        assert!(!numbers.fields[0].ty.constraints.nan);
        assert!(numbers.fields[1].ty.constraints.nan);
        assert!(!numbers.fields[1].ty.constraints.infinite);

        let schema = compiled(&[concat!(
            "namespace a\n",
            "message[in] M {\n",
            "    f: f32[min=0, max=1.5],\n",
            "    o: option<string[max_length=3]>,\n",
            "    m: map<u8[max=9], vec<i8[min=-1]>[min_length=1]>,\n",
            "}\n",
        )]);
        let (_, message) = def(&schema, "M");
        let DefKind::Message(message) = &message.kind else {
            panic!("expected a message");
        };
        let f = &message.fields[0].ty.constraints;
        assert_eq!(
            f.min,
            Some(Number::Float(0.0)),
            "integer bounds widen to floats"
        );
        assert_eq!(f.max, Some(Number::Float(1.5)));
        assert!(!f.infinite, "floats are finite by default");
        assert!(!f.nan, "floats are not NaN by default");
        assert!(message.fields[1].optional);
        assert_eq!(message.fields[1].ty.constraints.max_length, Some(3));
        let TypeKind::Map(key, value) = &message.fields[2].ty.kind else {
            panic!("expected a map");
        };
        assert_eq!(key.constraints.max, Some(Number::Int(9)));
        assert_eq!(value.constraints.min_length, Some(1));
    }

    #[test]
    fn constraints_are_validated() {
        let cases = [
            ("bool[min=0]", "`bool` takes no constraints"),
            (
                "array<i32, 2>[max_length=2]",
                "`array<i32, 2>` takes no constraints",
            ),
            (
                "i32[max_length=2]",
                "`max_length` is not a constraint of `i32`; expected one of `min`, `max`",
            ),
            (
                "string[max=2]",
                "`max` is not a constraint of `string`; expected one of `min_length`, `max_length`",
            ),
            (
                "i32[infinite]",
                "`infinite` is not a constraint of `i32`; expected one of `min`, `max`",
            ),
            ("i32[min=1, min=2]", "duplicate constraint `min`"),
            ("i32[min]", "`min` needs a value, as in `min=0`"),
            ("f32[infinite=1]", "`infinite` takes no value"),
            ("f32[nan=1]", "`nan` takes no value"),
            ("f32[nan, nan]", "duplicate constraint `nan`"),
            (
                "i32[nan]",
                "`nan` is not a constraint of `i32`; expected one of `min`, `max`",
            ),
            ("u8[max=256]", "`256` is out of range for `u8`"),
            ("u8[min=-1]", "`-1` is out of range for `u8`"),
            (
                "i32[min=0.5]",
                "`0.5` is not an integer; `i32` takes integer bounds",
            ),
            (
                "f32[max=1000000000000000000000000000000000000000.0]",
                "`1e39` is out of range for `f32`",
            ),
            (
                "vec<i32>[max_length=-1]",
                "`max_length` must be a non-negative integer",
            ),
            (
                "string[max_length=1.5]",
                "`max_length` must be a non-negative integer",
            ),
            ("i32[min=2, max=1]", "`max` is less than `min`"),
            ("f64[min=2.5, max=1]", "`max` is less than `min`"),
            (
                "bytes[min_length=2, max_length=1]",
                "`max_length` is less than `min_length`",
            ),
            (
                "option<i32>[min=0]",
                "an `option` takes no constraints; constrain its value instead, as in `option<i32[min=0]>`",
            ),
        ];
        for (ty, expected) in cases {
            let src = format!("namespace a\nmessage[in] M {{ x: {ty} }}\n");
            assert_eq!(messages(&[&src]), [expected], "{ty}");
        }
        // User types take no constraints either.
        assert_eq!(
            messages(&["namespace a\nenum E { A }\nmessage[in] M { e: E[max=1] }\n"]),
            ["`E` takes no constraints"]
        );
    }

    #[test]
    fn options_only_wrap_a_field() {
        assert_eq!(
            messages(&["namespace a\nmessage[in] M { v: vec<option<i32>> }\n"]),
            ["`option<i32>` is nested; `option` is only allowed as a field's own type"]
        );
        assert_eq!(
            messages(&["namespace a\nmessage[in] M { o: option<option<i32>> }\n"]),
            ["`option<i32>` is nested; `option` is only allowed as a field's own type"]
        );
    }

    #[test]
    fn unions_lower_their_variants_in_order() {
        let schema = compiled(&[concat!(
            "namespace a\n",
            "struct Item { id: u32 }\n",
            "union Op {\n",
            "    Add { item: Item, slot: u8, note: option<string> },\n",
            "    Remove(u8[max=9]),\n",
            "    Clear,\n",
            "}\n",
        )]);
        let (item_id, _) = def(&schema, "Item");
        let (_, op) = def(&schema, "Op");
        let DefKind::Union(op) = &op.kind else {
            panic!("expected a union");
        };
        assert_eq!(
            op.variants
                .iter()
                .map(|variant| variant.name.as_str())
                .collect::<Vec<_>>(),
            ["Add", "Remove", "Clear"]
        );
        let VariantData::Fields(fields) = &op.variants[0].data else {
            panic!("expected fields");
        };
        assert_eq!(fields[0].ty.kind, TypeKind::Named(item_id));
        assert_eq!(fields[1].ty.kind, TypeKind::U8);
        assert_eq!(fields[2].ty.kind, TypeKind::String);
        assert!(fields[2].optional);
        let VariantData::Type(ty) = &op.variants[1].data else {
            panic!("expected a type");
        };
        assert_eq!(ty.kind, TypeKind::U8);
        assert_eq!(ty.constraints.max, Some(Number::Int(9)));
        assert_eq!(op.variants[2].data, VariantData::Unit);
    }

    #[test]
    fn types_lower_onto_their_target() {
        let schema = compiled(&[concat!(
            "namespace a\n",
            "enum Color: u8 { Red }\n",
            "type PartyId = u64\n",
            "type Level = u8[max=100]\n",
            "type Hue = Color\n",
            "type Leader = PartyId\n",
            "message[in] M { type Local = bool p: PartyId, l: Local }\n",
        )]);
        let target = |name: &str| {
            let (_, def) = def(&schema, name);
            let DefKind::Type(item) = &def.kind else {
                panic!("expected a type");
            };
            item.target.clone()
        };
        assert_eq!(target("PartyId").kind, TypeKind::U64);
        assert_eq!(target("Level").kind, TypeKind::U8);
        assert_eq!(target("Level").constraints.max, Some(Number::Int(100)));
        let (color, _) = def(&schema, "Color");
        assert_eq!(target("Hue").kind, TypeKind::Named(color));
        let (party, _) = def(&schema, "PartyId");
        assert_eq!(target("Leader").kind, TypeKind::Named(party));

        let (message, _) = def(&schema, "M");
        let (local, local_def) = def(&schema, "Local");
        assert_eq!(local_def.parent, Some(message));
        assert_eq!(schema.path(local), ["M", "Local"]);

        // `resolved` follows every `type` to its scalar.
        let leader = Type::from(TypeKind::Named(def(&schema, "Leader").0));
        assert_eq!(schema.resolved(&leader).kind, TypeKind::U64);
        let level = Type::from(TypeKind::Named(def(&schema, "Level").0));
        assert_eq!(
            schema.resolved(&level).constraints.max,
            Some(Number::Int(100))
        );
    }

    #[test]
    fn unions_are_validated() {
        let cases = [
            ("union U {}", "union `U` needs at least one variant"),
            ("union U { A, B(u8), A }", "duplicate variant `A`"),
            (
                "union U { Add {} }",
                "variant `Add` has no fields; write it as `Add`",
            ),
            ("union U { A { x: u8, x: u16 } }", "duplicate field `x`"),
            (
                "union U { A(option<u8>) }",
                "`option<u8>` is nested; `option` is only allowed as a field's own type",
            ),
            (
                "union U { A { v: option<vec<u8>> } }",
                "`vec<u8>` is a collection; collections cannot be optional",
            ),
            (
                "union U { A(Missing) }",
                "cannot find `Missing` in this scope",
            ),
            (
                "message[in] M {}\nunion U { A(M) }",
                "`M` is a message; messages are standalone and cannot be used as a type",
            ),
        ];
        for (src, expected) in cases {
            assert_eq!(
                messages(&[&format!("namespace a\n{src}\n")]),
                [expected],
                "{src}"
            );
        }
    }

    #[test]
    fn unions_hold_at_most_65535_variants() {
        let union = |count: usize| {
            let variants: Vec<String> = (0..count).map(|index| format!("V{index}")).collect();
            format!("namespace a\nunion U {{ {} }}\n", variants.join(", "))
        };
        assert_eq!(
            messages(&[&union(65536)]),
            ["union `U` has 65536 variants; a union holds at most 65535"]
        );
        compiled(&[&union(65535)]);
    }

    #[test]
    fn types_stand_for_scalars() {
        let scalar = "is not a scalar; a `type` stands for an integer, a float, `bool`, \
                      `timestamp`, `duration`, `entity`, `uentity`, `uuid`, an enum, or another `type`";
        let prelude = "namespace a\nstruct S { x: u8 }\nunion U { A }\n";
        for target in [
            "string",
            "string[min_length=2, max_length=2]",
            "bytes",
            "vec<u8>",
            "set<u8>",
            "map<u8, u8>",
            "array<u8, 2>",
            "S",
            "U",
        ] {
            assert_eq!(
                messages(&[&format!("{prelude}type T = {target}\n")]),
                [format!("`{target}` {scalar}")],
                "{target}"
            );
        }
        assert_eq!(
            messages(&[&format!("{prelude}type T = option<u8>\n")]),
            ["a `type` cannot be optional; make the field holding it optional instead"]
        );
        // Through another `type`, it is the target that is checked.
        assert_eq!(
            messages(&[&format!("{prelude}type T = string\ntype V = T\n")]),
            [format!("`string` {scalar}")]
        );

        compiled(&[concat!(
            "namespace a\n",
            "enum E { A }\n",
            "type A = u8\n",
            "type B = i64[min=-5, max=5]\n",
            "type C = f32[nan]\n",
            "type D = f64\n",
            "type F = bool\n",
            "type G = timestamp\n",
            "type H = duration\n",
            "type I = entity\n",
            "type J = uentity\n",
            "type K = uuid\n",
            "type L = E\n",
            "type M = A\n",
        )]);
    }

    #[test]
    fn named_types_take_no_constraints() {
        let prelude = "namespace a\ntype PartyId = u64\nunion U { A }\n";
        let cases = [
            ("p: PartyId[max=3]", "`PartyId` takes no constraints"),
            ("v: vec<PartyId[min=1]>", "`PartyId` takes no constraints"),
            ("u: U[max_length=3]", "`U` takes no constraints"),
        ];
        for (field, expected) in cases {
            let src = format!("{prelude}message[in] M {{ {field} }}\n");
            assert_eq!(messages(&[&src]), [expected], "{field}");
        }
        assert_eq!(
            messages(&[&format!("{prelude}union V {{ A(PartyId[max=3]) }}\n")]),
            ["`PartyId` takes no constraints"]
        );
        assert_eq!(
            messages(&[&format!("{prelude}type T = PartyId[max=3]\n")]),
            ["`PartyId` takes no constraints"]
        );
    }

    #[test]
    fn unions_are_variable_size_and_types_are_their_target() {
        let prelude = concat!(
            "namespace a\n",
            "union U { A }\n",
            "type PartyId = u64\n",
            "type Weight = f32\n",
            "struct Holder { u: U }\n",
        );
        let cases = [
            (
                "a: array<U, 2>",
                "`U` is variable-size; an array item must be fixed-size",
            ),
            (
                "s: set<U>",
                "`U` is variable-size; a set item must be fixed-size",
            ),
            (
                "m: map<U, u8>",
                "`U` is variable-size; a map key must be fixed-size",
            ),
            (
                "a: array<Holder, 2>",
                "`Holder` is variable-size; an array item must be fixed-size",
            ),
            (
                "s: set<Weight>",
                "`Weight` holds a float; a set item cannot hold a float",
            ),
            (
                "m: map<Weight, u8>",
                "`Weight` holds a float; a map key cannot hold a float",
            ),
        ];
        for (field, expected) in cases {
            let src = format!("{prelude}message[in] M {{ {field} }}\n");
            assert_eq!(messages(&[&src]), [expected], "{field}");
        }

        // A union is fine wherever a variable-size value is, and a `type`
        // wherever its target is.
        compiled(&[&format!(
            "{prelude}message[in] M {{ v: vec<U>, m: map<u8, U>, o: option<U>, \
             a: array<PartyId, 2>, s: set<PartyId>, k: map<PartyId, Weight>, \
             w: vec<Weight>, p: option<PartyId> }}\n"
        )]);
    }

    #[test]
    fn wire_errors_in_unions_point_at_the_variant() {
        let src = concat!(
            "namespace a\n",
            "union U {\n",
            "    Big(array<u64, 9000>),\n",
            "    Wide { a: u8, b: array<u64, 9000> },\n",
            "    Long { a: array<u64, 5000>, b: array<u64, 5000> },\n",
            "}\n",
        );
        let errors: Vec<_> = compile(&[src])
            .expect_err("expected errors")
            .into_iter()
            .filter(|diagnostic| diagnostic.is_error())
            .map(|diagnostic| (src[diagnostic.span.clone()].to_string(), diagnostic.message))
            .collect();
        assert_eq!(
            errors,
            [
                (
                    "array<u64, 9000>".to_string(),
                    "`array<u64, 9000>` takes 72000 bytes; a message holds at most 65535"
                        .to_string()
                ),
                (
                    "array<u64, 9000>".to_string(),
                    "`array<u64, 9000>` takes 72000 bytes; a message holds at most 65535"
                        .to_string()
                ),
                (
                    "Long".to_string(),
                    "`a.U.Long` takes at least 80000 bytes; a message holds at most 65535"
                        .to_string()
                ),
            ]
        );
    }

    #[test]
    fn recursion_is_reported() {
        let recursive = "`S` refers to itself; recursive types are not allowed";
        assert_eq!(messages(&["namespace a\nstruct S { s: S }\n"]), [recursive]);
        // Collections and options don't make it acceptable.
        assert_eq!(
            messages(&["namespace a\nstruct S { x: u8, s: vec<S> }\n"]),
            [recursive]
        );
        assert_eq!(
            messages(&["namespace a\nstruct S { x: u8, s: map<u8, S> }\n"]),
            [recursive]
        );
        assert_eq!(
            messages(&["namespace a\nstruct A { b: B }\nstruct B { a: array<A, 2> }\n"]).len(),
            1
        );
        assert_eq!(
            messages(&["namespace a\nstruct A { b: option<B> }\nstruct B { a: vec<A> }\n"]).len(),
            1
        );

        compiled(&["namespace a\nstruct S { x: i32 }\nmessage[in] M { v: vec<S> }\n"]);
    }

    #[test]
    fn recursion_through_unions_and_types_is_reported() {
        let cases = [
            ("union U { A(U) }", "U"),
            ("union U { A, B { v: vec<U> } }", "U"),
            ("union U { A(map<u8, U>) }", "U"),
            ("union U { A(S) }\nstruct S { u: option<U> }", "U"),
            ("type A = A", "A"),
            ("type A = B\ntype B = A", "A"),
        ];
        for (src, name) in cases {
            assert_eq!(
                messages(&[&format!("namespace a\n{src}\n")]),
                [format!(
                    "`{name}` refers to itself; recursive types are not allowed"
                )],
                "{src}"
            );
        }
        // Unions and types may refer to others, as long as nothing loops.
        compiled(&[concat!(
            "namespace a\n",
            "type Id = u32\n",
            "union Inner { A(Id), B }\n",
            "union Outer { I(Inner), L { all: vec<Inner>, id: Id } }\n",
        )]);
    }
}
