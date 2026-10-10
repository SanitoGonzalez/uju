use std::collections::HashSet;

use crate::diagnostic::{Diagnostic, SourceId};
use crate::ir::*;
use crate::lexer::Span;
use crate::parser::ast;
use crate::resolve::{Resolver, TypeSite};

/// Lower parsed schemas into the IR, resolving every name and validating
/// every declaration. `schemas` are compiled together and indexed by
/// [`SourceId`]; the IR is only produced when no errors were found.
pub fn lower(schemas: &[ast::Schema<'_>]) -> Result<Schema, Vec<Diagnostic>> {
    let (resolver, diagnostics) = Resolver::new(schemas);
    let mut lowerer = Lowerer {
        resolver: &resolver,
        diagnostics,
    };

    let defs: Vec<Def> = resolver
        .types
        .iter()
        .enumerate()
        .map(|(index, site)| lowerer.lower_def(TypeId(index), site))
        .collect();

    // Only meaningful once every type has resolved; unresolved fields carry
    // placeholder types that would produce nonsense cycles.
    if lowerer.diagnostics.is_empty() {
        lowerer.check_cycles(&defs);
    }
    if !lowerer.diagnostics.is_empty() {
        return Err(lowerer.diagnostics);
    }

    Ok(Schema {
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
    })
}

struct Lowerer<'a, 'src> {
    resolver: &'a Resolver<'a, 'src>,
    diagnostics: Vec<Diagnostic>,
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
        let kind = match site.item {
            ast::Item::Enum(item) => DefKind::Enum(self.lower_enum(site.source, item)),
            ast::Item::Struct(item) => DefKind::Struct(self.lower_struct(site.source, id, item)),
            ast::Item::Message(item) => DefKind::Message(self.lower_message(site.source, id, item)),
        };
        Def {
            namespace: site.namespace,
            parent: site.parent,
            name: site.item.name().0.to_string(),
            kind,
        }
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
        body: &ast::Body<'src>,
    ) -> Vec<Field> {
        let mut seen = HashSet::new();
        body.fields
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
        let fields = self.lower_fields(source, id, &item.body);
        for (field, ast_field) in fields.iter().zip(&item.body.fields) {
            if field.optional {
                self.diagnostics.push(Diagnostic::new(
                    source,
                    ast_field.name.1.clone(),
                    "a struct field cannot be optional",
                ));
            }
            if !self.is_fixed(&field.ty) {
                self.diagnostics.push(Diagnostic::new(
                    source,
                    ast_field.ty.span.clone(),
                    format!(
                        "`{}` is variable-size; a struct field must be fixed-size",
                        ast_field.ty
                    ),
                ));
            }
        }
        Struct { fields }
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
            fields: self.lower_fields(source, id, &item.body),
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
                if !self.is_fixed(&item_ty) {
                    self.error(
                        source,
                        &item.span,
                        format!("`{item}` is variable-size; an array item must be fixed-size"),
                    );
                }
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
        role: &str,
    ) {
        if !self.is_fixed(key_ty) {
            self.error(
                source,
                &key.span,
                format!("`{key}` is variable-size; {role} must be fixed-size"),
            );
        } else if let Some(path) = self.holds_float(source, scope, key, &mut Vec::new()) {
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
    /// directly (an empty path) or through a struct or array. `visiting`
    /// guards against recursive structs, which are reported separately.
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
                let ast::Item::Struct(item) = site.item else {
                    return None;
                };
                if visiting.contains(&id) {
                    return None;
                }
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

    fn is_fixed(&self, ty: &Type) -> bool {
        match &ty.kind {
            TypeKind::String
            | TypeKind::Bytes
            | TypeKind::Vec(_)
            | TypeKind::Set(_)
            | TypeKind::Map(..) => false,
            TypeKind::Named(id) => matches!(
                self.resolver.types[id.0].item,
                ast::Item::Enum(_) | ast::Item::Struct(_)
            ),
            _ => true,
        }
    }

    /// Report types whose fixed parts recursively contain themselves; such a
    /// type has no finite size. Variable-size types (`vec`, `string`, ...)
    /// break a cycle, since their contents live out of line.
    /// Optional fields do not: their data still sits inline behind a
    /// presence bit.
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
        let fields = match &defs[index].kind {
            DefKind::Enum(_) => &[][..],
            DefKind::Struct(item) => &item.fields,
            DefKind::Message(item) => &item.fields,
        };
        let mut targets = Vec::new();
        for field in fields {
            inline_refs(&field.ty, &mut targets);
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
                            "`{}` recursively contains itself and has no finite size; \
                             break the cycle with `vec` or another variable-size container",
                            name.0
                        ),
                    ));
                }
            }
        }
        marks[index] = Mark::Black;
    }
}

/// The named types a value of `ty` stores inline, i.e. not behind a
/// variable-size container.
fn inline_refs(ty: &Type, out: &mut Vec<TypeId>) {
    match &ty.kind {
        TypeKind::Named(id) => out.push(*id),
        TypeKind::Array(item, _) => inline_refs(item, out),
        // Scalars reference nothing; vec/set/map/string/bytes contents are
        // out of line.
        _ => {}
    }
}

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
        ast::Prim::Interval => TypeKind::Interval,
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

    const SAMPLE1: &str = include_str!("../../tests/data/sample1.uju");
    const SAMPLE2: &str = include_str!("../../tests/data/sample2.uju");

    fn compiled(sources: &[&str]) -> Schema {
        compile(sources).unwrap_or_else(|errors| panic!("unexpected errors: {errors:#?}"))
    }

    fn messages(sources: &[&str]) -> Vec<String> {
        compile(sources)
            .expect_err("expected errors")
            .into_iter()
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
    fn samples_lower() {
        let schema = compiled(&[SAMPLE1, SAMPLE2]);
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
    }

    #[test]
    fn messages_carry_their_direction() {
        let schema = compiled(&[SAMPLE1]);
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
            "namespace a;\n",
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
        let schema = compiled(&[SAMPLE1, SAMPLE2]);
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
        let src = "namespace a;\nenum R { X }\nmessage[in] M {\n    enum R { Y }\n    r: R,\n}\n";
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
        compiled(&["namespace a;\nmessage[in] M { enum R { Y } r: R, }\nstruct S { r: M.R }\n"]);
    }

    #[test]
    fn unknown_names_are_reported() {
        assert_eq!(
            messages(&["namespace a;\nstruct S { p: Position }\n"]),
            ["cannot find `Position` in this scope"]
        );
        assert_eq!(
            messages(&["namespace a;\nuse b;\n"]),
            ["unknown namespace `b`"]
        );
        assert_eq!(
            messages(&[
                "namespace a;\nstruct T {}\n",
                "namespace b;\nuse a;\nstruct S { t: a.X }\n",
            ]),
            ["no `X` in namespace `a`"]
        );
        assert_eq!(
            messages(&["namespace a;\nenum E { X }\nstruct S { e: E.X }\n"]),
            ["`E` has no nested declaration `X`"]
        );
    }

    #[test]
    fn qualified_paths_need_a_use() {
        let a = "namespace a;\nstruct T {}\n";
        let b = "namespace b;\nstruct S { t: a.T }\n";
        assert_eq!(messages(&[a, b]), ["cannot find `a` in this scope"]);
    }

    #[test]
    fn unqualified_imports_can_be_ambiguous() {
        let a = "namespace a;\nstruct T {}\n";
        let b = "namespace b;\nstruct T {}\n";
        let c = "namespace c;\nuse a;\nuse b;\nstruct S { t: T }\n";
        let messages = messages(&[a, b, c]);
        assert_eq!(
            messages,
            ["`T` is ambiguous; it is declared in namespaces `a` and `b`"]
        );
    }

    #[test]
    fn duplicate_declarations_are_reported() {
        assert_eq!(
            messages(&["namespace a;\nstruct S {}\nenum S { A }\n"]),
            ["the name `S` is declared more than once in this scope"]
        );
        assert_eq!(
            messages(&["namespace a;\nstruct S { x: i32, x: i32 }\n"]),
            ["duplicate field `x`"]
        );
        // The same name in two files sharing a namespace also collides.
        assert_eq!(
            messages(&["namespace a;\nstruct S {}\n", "namespace a;\nstruct S {}\n"]),
            ["the name `S` is declared more than once in this scope"]
        );
    }

    #[test]
    fn enums_are_validated() {
        assert_eq!(
            messages(&["namespace a;\nenum E: i32 { A }\n"]),
            ["enum `E` cannot be backed by `i32`; only unsigned integers are allowed"]
        );
        assert_eq!(
            messages(&["namespace a;\nenum E: u8 { A = 256 }\n"]),
            ["`256` is out of range for `u8`"]
        );
        assert_eq!(
            messages(&["namespace a;\nenum E: u8 { A = 255, B }\n"]),
            ["variant value exceeds the maximum of `u8`"]
        );
        assert_eq!(
            messages(&["namespace a;\nenum E { A, B = 0 }\n"]),
            ["`A` and `B` share the value 0"]
        );
        assert_eq!(
            messages(&["namespace a;\nenum E { A, A }\n"]),
            ["duplicate variant `A`"]
        );
        assert_eq!(
            messages(&["namespace a;\nenum E {}\n"]),
            ["enum `E` needs at least one variant"]
        );
    }

    #[test]
    fn messages_are_standalone() {
        assert_eq!(
            messages(&["namespace a;\nmessage[in] M {}\nstruct S { m: M }\n"]),
            ["`M` is a message; messages are standalone and cannot be used as a type"]
        );
        assert_eq!(
            messages(&["namespace a;\nmessage[in] M {}\nmessage[out] N { v: vec<M> }\n"]).len(),
            1
        );
    }

    #[test]
    fn structs_are_fixed_size_pods() {
        assert_eq!(
            messages(&["namespace a;\nstruct S { v: vec<i32> }\n"]),
            ["`vec<i32>` is variable-size; a struct field must be fixed-size"]
        );
        assert_eq!(
            messages(&["namespace a;\nstruct S { s: string }\n"]),
            ["`string` is variable-size; a struct field must be fixed-size"]
        );
        assert_eq!(
            messages(&["namespace a;\nstruct S { x: option<i32> }\n"]),
            ["a struct field cannot be optional"]
        );
    }

    #[test]
    fn containers_need_fixed_size_items() {
        assert_eq!(
            messages(&["namespace a;\nmessage[in] M { a: array<vec<i32>, 2> }\n"]),
            ["`vec<i32>` is variable-size; an array item must be fixed-size"]
        );
        assert_eq!(
            messages(&["namespace a;\nmessage[in] M { s: set<string> }\n"]),
            ["`string` is variable-size; a set item must be fixed-size"]
        );
        assert_eq!(
            messages(&["namespace a;\nmessage[in] M { m: map<string, i32> }\n"]),
            ["`string` is variable-size; a map key must be fixed-size"]
        );
        // Map values may be variable-size.
        compiled(&["namespace a;\nmessage[in] M { m: map<i32, vec<i32>> }\n"]);
    }

    #[test]
    fn collections_cannot_be_optional() {
        for ty in ["vec<i32>", "set<i32>", "map<i32, i32>", "array<i32, 2>"] {
            assert_eq!(
                messages(&[&format!(
                    "namespace a;\nmessage[in] M {{ o: option<{ty}> }}\n"
                )]),
                [format!(
                    "`{ty}` is a collection; collections cannot be optional"
                )]
            );
        }
        // Variable-size scalars and user types may be.
        compiled(&[concat!(
            "namespace a;\n",
            "struct P { x: f32 }\n",
            "enum E { A }\n",
            "message[in] M { s: option<string>, b: option<bytes>, p: option<P>, e: option<E> }\n",
        )]);
    }

    #[test]
    fn floats_cannot_be_keys() {
        let prelude = concat!(
            "namespace a;\n",
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
            "namespace a;\n",
            "struct Cell { x: i32, y: i32 }\n",
            "message[in] M { m: map<Cell, f32[nan]>, s: set<Cell> }\n",
        )]);
    }

    #[test]
    fn constraints_lower_onto_the_type_they_follow() {
        let schema = compiled(&[SAMPLE1]);
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
            "namespace a;\n",
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
            let src = format!("namespace a;\nmessage[in] M {{ x: {ty} }}\n");
            assert_eq!(messages(&[&src]), [expected], "{ty}");
        }
        // User types take no constraints either.
        assert_eq!(
            messages(&["namespace a;\nenum E { A }\nmessage[in] M { e: E[max=1] }\n"]),
            ["`E` takes no constraints"]
        );
    }

    #[test]
    fn options_only_wrap_a_field() {
        assert_eq!(
            messages(&["namespace a;\nmessage[in] M { v: vec<option<i32>> }\n"]),
            ["`option<i32>` is nested; `option` is only allowed as a field's own type"]
        );
        assert_eq!(
            messages(&["namespace a;\nmessage[in] M { o: option<option<i32>> }\n"]),
            ["`option<i32>` is nested; `option` is only allowed as a field's own type"]
        );
    }

    #[test]
    fn infinite_recursion_is_reported() {
        assert_eq!(
            messages(&["namespace a;\nstruct S { s: S }\n"]),
            ["`S` recursively contains itself and has no finite size; \
              break the cycle with `vec` or another variable-size container"]
        );
        assert_eq!(
            messages(&["namespace a;\nstruct A { b: B }\nstruct B { a: array<A, 2> }\n"]).len(),
            1
        );

        // Variable-size types break the cycle.
        compiled(&["namespace a;\nstruct S { x: i32 }\nmessage[in] M { v: vec<S> }\n"]);
    }
}
