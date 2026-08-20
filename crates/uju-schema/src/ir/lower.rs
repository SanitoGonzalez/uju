use std::collections::HashSet;

use crate::diagnostic::{Diagnostic, SourceId};
use crate::ir::*;
use crate::lexer::{Span, unescape};
use crate::parser::ast;
use crate::resolve::{Resolution, Resolver, TypeSite};

/// Lower parsed schemas into the IR, resolving every name and validating
/// every declaration. `schemas` are compiled together and indexed by
/// [`SourceId`]; the IR is only produced when no errors were found.
pub fn lower(schemas: &[ast::Schema<'_>]) -> Result<Schema, Vec<Diagnostic>> {
    let (resolver, diagnostics) = Resolver::new(schemas);
    let mut lowerer = Lowerer {
        resolver: &resolver,
        diagnostics,
        consts: vec![ConstState::Todo; resolver.consts.len()],
    };

    let defs: Vec<Def> = resolver
        .types
        .iter()
        .enumerate()
        .map(|(index, site)| lowerer.lower_def(TypeId(index), site))
        .collect();

    for index in 0..resolver.consts.len() {
        lowerer.const_value(ConstId(index));
    }

    // Only meaningful once every type has resolved; unresolved fields carry
    // placeholder types that would produce nonsense cycles.
    if lowerer.diagnostics.is_empty() {
        lowerer.check_cycles(&defs);
    }
    if !lowerer.diagnostics.is_empty() {
        return Err(lowerer.diagnostics);
    }

    let consts = resolver
        .consts
        .iter()
        .zip(lowerer.consts)
        .map(|(site, state)| {
            let ConstState::Done(Some((ty, value))) = state else {
                unreachable!("a failed constant reports a diagnostic");
            };
            Const {
                namespace: site.namespace,
                parent: site.parent,
                name: site.konst.name.0.to_string(),
                ty,
                value,
            }
        })
        .collect();

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
        consts,
    })
}

struct Lowerer<'a, 'src> {
    resolver: &'a Resolver<'a, 'src>,
    diagnostics: Vec<Diagnostic>,
    consts: Vec<ConstState>,
}

#[derive(Clone)]
enum ConstState {
    Todo,
    InProgress,
    Done(Option<(Type, Value)>),
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
            ast::Item::Union(item) => DefKind::Union(self.lower_union(site.source, id, item)),
            ast::Item::Message(item) => DefKind::Message(self.lower_message(site.source, id, item)),
            ast::Item::Const(_) => unreachable!("constants are collected separately"),
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
                Field {
                    name: field.name.0.to_string(),
                    ty: self.lower_type(source, Some(scope), &field.ty),
                    optional: field.optional,
                }
            })
            .collect()
    }

    fn lower_struct(&mut self, source: SourceId, id: TypeId, item: &ast::Struct<'src>) -> Struct {
        let fields = self.lower_fields(source, id, &item.body);
        for (field, ast_field) in fields.iter().zip(&item.body.fields) {
            if ast_field.optional {
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

    fn lower_union(&mut self, source: SourceId, id: TypeId, item: &ast::Union<'src>) -> Union {
        if item.body.fields.is_empty() {
            self.error(
                source,
                &item.name.1,
                format!("union `{}` needs at least one member", item.name.0),
            );
        }
        for field in &item.body.fields {
            if field.optional {
                self.diagnostics.push(Diagnostic::new(
                    source,
                    field.name.1.clone(),
                    "a union member cannot be optional",
                ));
            }
        }
        Union {
            members: self.lower_fields(source, id, &item.body),
        }
    }

    fn lower_message(
        &mut self,
        source: SourceId,
        id: TypeId,
        item: &ast::Message<'src>,
    ) -> Message {
        let response = item.response.as_ref().and_then(|path| {
            match self.resolver.resolve(source, Some(id), path) {
                Ok(Resolution::Type(target))
                    if matches!(self.resolver.types[target.0].item, ast::Item::Message(_)) =>
                {
                    Some(target)
                }
                Ok(_) => {
                    self.error(source, &path.span, format!("`{path}` is not a message"));
                    None
                }
                Err(diagnostic) => {
                    self.diagnostics.push(diagnostic);
                    None
                }
            }
        });
        Message {
            response,
            fields: self.lower_fields(source, id, &item.body),
        }
    }

    fn lower_type(
        &mut self,
        source: SourceId,
        scope: Option<TypeId>,
        ty: &ast::Type<'src>,
    ) -> Type {
        match &ty.kind {
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
                Type::Array(Box::new(item_ty), *len)
            }
            ast::TypeKind::Vec(item) => Type::Vec(Box::new(self.lower_type(source, scope, item))),
            ast::TypeKind::Set(item) => {
                let item_ty = self.lower_type(source, scope, item);
                if !self.is_fixed(&item_ty) {
                    self.error(
                        source,
                        &item.span,
                        format!("`{item}` is variable-size; a set item must be fixed-size"),
                    );
                }
                Type::Set(Box::new(item_ty))
            }
            ast::TypeKind::Map(key, value) => {
                let key_ty = self.lower_type(source, scope, key);
                if !self.is_fixed(&key_ty) {
                    self.error(
                        source,
                        &key.span,
                        format!("`{key}` is variable-size; a map key must be fixed-size"),
                    );
                }
                Type::Map(
                    Box::new(key_ty),
                    Box::new(self.lower_type(source, scope, value)),
                )
            }
            ast::TypeKind::Named(path) => match self.resolver.resolve(source, scope, path) {
                Ok(Resolution::Type(id)) => {
                    if matches!(self.resolver.types[id.0].item, ast::Item::Message(_)) {
                        self.error(
                            source,
                            &ty.span,
                            format!(
                                "`{path}` is a message; messages are standalone and cannot be used as a type"
                            ),
                        );
                        Type::Bool
                    } else {
                        Type::Named(id)
                    }
                }
                Ok(_) => {
                    self.error(source, &ty.span, format!("`{path}` is not a type"));
                    Type::Bool
                }
                Err(diagnostic) => {
                    self.diagnostics.push(diagnostic);
                    Type::Bool
                }
            },
        }
    }

    fn is_fixed(&self, ty: &Type) -> bool {
        match ty {
            Type::String | Type::Bytes | Type::Vec(_) | Type::Set(_) | Type::Map(..) => false,
            Type::Named(id) => matches!(
                self.resolver.types[id.0].item,
                ast::Item::Enum(_) | ast::Item::Struct(_)
            ),
            _ => true,
        }
    }

    /// Evaluate a constant, memoizing so that constants referencing other
    /// constants are computed once and cycles are caught.
    fn const_value(&mut self, id: ConstId) -> Option<(Type, Value)> {
        match &self.consts[id.0] {
            ConstState::Done(result) => result.clone(),
            ConstState::InProgress => {
                let resolver = self.resolver;
                let name = &resolver.consts[id.0].konst.name;
                self.error(
                    resolver.consts[id.0].source,
                    &name.1.clone(),
                    format!("constant `{}` is defined in terms of itself", name.0),
                );
                self.consts[id.0] = ConstState::Done(None);
                None
            }
            ConstState::Todo => {
                self.consts[id.0] = ConstState::InProgress;
                let result = self.eval_const(id);
                self.consts[id.0] = ConstState::Done(result.clone());
                result
            }
        }
    }

    fn eval_const(&mut self, id: ConstId) -> Option<(Type, Value)> {
        let resolver = self.resolver;
        let site = &resolver.consts[id.0];
        let konst = site.konst;

        let before = self.diagnostics.len();
        let ty = self.lower_type(site.source, site.parent, &konst.ty);
        if self.diagnostics.len() > before {
            return None;
        }

        let allowed = match &ty {
            Type::Bool | Type::String | Type::F32 | Type::F64 => true,
            Type::Named(target) => matches!(resolver.types[target.0].item, ast::Item::Enum(_)),
            _ => int_bounds(&ty).is_some(),
        };
        if !allowed {
            self.error(
                site.source,
                &konst.ty.span,
                format!("`{}` cannot be the type of a constant", konst.ty),
            );
            return None;
        }

        let (value, span) = &konst.value;
        let value = match value {
            ast::Value::Bool(value) => Value::Bool(*value),
            ast::Value::Int(value) => Value::Int(*value),
            ast::Value::Float(value) => Value::Float(*value),
            ast::Value::Str(contents) => Value::Str(unescape(contents).into_owned()),
            ast::Value::Path(path) => match resolver.resolve(site.source, site.parent, path) {
                Ok(Resolution::Variant(target, variant)) => Value::EnumVariant(target, variant),
                Ok(Resolution::Const(other)) => self.const_value(other)?.1,
                Ok(Resolution::Type(_)) => {
                    self.error(
                        site.source,
                        span,
                        format!("`{path}` is a type, not a value"),
                    );
                    return None;
                }
                Err(diagnostic) => {
                    self.diagnostics.push(diagnostic);
                    return None;
                }
            },
        };

        let value = self.coerce(site.source, span, &konst.ty, &ty, value)?;
        Some((ty, value))
    }

    /// Check `value` against the constant's type, coercing integer literals
    /// into floats. `expected_ast` is only used to word errors as written.
    fn coerce(
        &mut self,
        source: SourceId,
        span: &Span,
        expected_ast: &ast::Type<'src>,
        expected: &Type,
        value: Value,
    ) -> Option<Value> {
        match (&value, expected) {
            (Value::Bool(_), Type::Bool) => Some(value),
            (Value::Str(_), Type::String) => Some(value),
            (Value::Int(int), _) if int_bounds(expected).is_some() => {
                let (min, max) = int_bounds(expected).unwrap();
                if (min..=max).contains(int) {
                    Some(value)
                } else {
                    self.error(
                        source,
                        span,
                        format!("`{int}` is out of range for `{expected_ast}`"),
                    );
                    None
                }
            }
            (Value::Int(int), Type::F32 | Type::F64) => Some(Value::Float(*int as f64)),
            (Value::Float(_), Type::F64) => Some(value),
            (Value::Float(float), Type::F32) => {
                if (*float as f32).is_finite() {
                    Some(value)
                } else {
                    self.error(source, span, format!("`{float}` is out of range for `f32`"));
                    None
                }
            }
            (Value::EnumVariant(actual, _), Type::Named(target)) if actual == target => Some(value),
            _ => {
                let found = self.describe(&value);
                self.error(
                    source,
                    span,
                    format!("mismatched types: expected `{expected_ast}`, found {found}"),
                );
                None
            }
        }
    }

    fn describe(&self, value: &Value) -> String {
        match value {
            Value::Bool(_) => "a boolean".to_string(),
            Value::Int(_) => "an integer".to_string(),
            Value::Float(_) => "a float".to_string(),
            Value::Str(_) => "a string".to_string(),
            Value::EnumVariant(id, _) => {
                format!(
                    "a variant of enum `{}`",
                    self.resolver.types[id.0].item.name().0
                )
            }
        }
    }

    /// Report types whose fixed parts recursively contain themselves; such a
    /// type has no finite wire size. Variable-size types (`vec`, `string`,
    /// unions, ...) break a cycle, since their contents live out of line.
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
            DefKind::Enum(_) | DefKind::Union(_) => &[][..],
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
    match ty {
        Type::Named(id) => out.push(*id),
        Type::Array(item, _) => inline_refs(item, out),
        // Scalars reference nothing; vec/set/map/string/bytes contents are
        // out of line.
        _ => {}
    }
}

fn int_bounds(ty: &Type) -> Option<(i128, i128)> {
    Some(match ty {
        Type::I8 => (i8::MIN as i128, i8::MAX as i128),
        Type::I16 => (i16::MIN as i128, i16::MAX as i128),
        Type::I32 => (i32::MIN as i128, i32::MAX as i128),
        Type::I64 => (i64::MIN as i128, i64::MAX as i128),
        Type::U8 => (0, u8::MAX as i128),
        Type::U16 => (0, u16::MAX as i128),
        Type::U32 => (0, u32::MAX as i128),
        Type::U64 => (0, u64::MAX as i128),
        _ => return None,
    })
}

fn prim_type(prim: ast::Prim) -> Type {
    match prim {
        ast::Prim::I8 => Type::I8,
        ast::Prim::I16 => Type::I16,
        ast::Prim::I32 => Type::I32,
        ast::Prim::I64 => Type::I64,
        ast::Prim::U8 => Type::U8,
        ast::Prim::U16 => Type::U16,
        ast::Prim::U32 => Type::U32,
        ast::Prim::U64 => Type::U64,
        ast::Prim::F32 => Type::F32,
        ast::Prim::F64 => Type::F64,
        ast::Prim::Bool => Type::Bool,
        ast::Prim::Timestamp => Type::Timestamp,
        ast::Prim::Interval => Type::Interval,
        ast::Prim::Entity => Type::Entity,
        ast::Prim::UEntity => Type::UEntity,
        ast::Prim::Uuid => Type::Uuid,
        ast::Prim::String => Type::String,
        ast::Prim::Bytes => Type::Bytes,
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

    fn konst<'a>(schema: &'a Schema, name: &str) -> &'a Const {
        schema
            .consts
            .iter()
            .find(|konst| konst.name == name)
            .unwrap_or_else(|| panic!("no constant named `{name}`"))
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

        let (color_id, color) = def(&schema, "Color");
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

        assert_eq!(konst(&schema, "NumberOne").ty, Type::I32);
        assert_eq!(konst(&schema, "NumberOne").value, Value::Int(1));
        assert_eq!(konst(&schema, "FavoriteColor").ty, Type::Named(color_id));
        assert_eq!(
            konst(&schema, "FavoriteColor").value,
            Value::EnumVariant(color_id, 0)
        );
    }

    #[test]
    fn messages_link_to_their_response() {
        let schema = compiled(&[SAMPLE1]);
        let (response_id, _) = def(&schema, "MyResponse");
        let (_, request) = def(&schema, "MyRequest");
        let DefKind::Message(request) = &request.kind else {
            panic!("expected a message");
        };
        assert_eq!(request.response, Some(response_id));

        let (result_id, result) = def(&schema, "Result");
        assert_eq!(result.parent, Some(response_id));
        assert_eq!(schema.path(result_id), ["MyResponse", "Result"]);

        let (_, response) = def(&schema, "MyResponse");
        let DefKind::Message(response) = &response.kind else {
            panic!("expected a message");
        };
        assert_eq!(response.response, None);
        assert_eq!(response.fields[0].ty, Type::Named(result_id));
    }

    #[test]
    fn container_types_lower() {
        let schema = compiled(&[SAMPLE1]);
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

        assert_eq!(field("ar").ty, Type::Array(Box::new(Type::I32), 4));
        assert_eq!(
            field("m2").ty,
            Type::Map(
                Box::new(Type::U8),
                Box::new(Type::Vec(Box::new(Type::Named(position_id))))
            )
        );
        assert!(field("vo").optional);
        assert!(!field("v").optional);
    }

    #[test]
    fn imports_resolve_across_sources() {
        let schema = compiled(&[SAMPLE1, SAMPLE2]);
        let (velocity_id, velocity) = def(&schema, "Velocity");
        let (position_id, _) = def(&schema, "Position");
        assert_eq!(
            schema.namespace(velocity.namespace).name.join("."),
            "foo.bar"
        );

        let (_, external) = def(&schema, "ExternalTypes");
        let DefKind::Message(external) = &external.kind else {
            panic!("expected a message");
        };
        assert_eq!(external.fields[0].ty, Type::Named(velocity_id));
        assert_eq!(external.fields[1].ty, Type::Named(position_id));
    }

    #[test]
    fn nested_declarations_shadow_outer_ones() {
        let src = "namespace a;\nenum R { X }\nmessage M {\n    enum R { Y }\n    r: R,\n}\n";
        let schema = compiled(&[src]);
        let (_, message) = def(&schema, "M");
        let DefKind::Message(message) = &message.kind else {
            panic!("expected a message");
        };
        let Type::Named(id) = message.fields[0].ty else {
            panic!("expected a named type");
        };
        assert!(schema.def(id).parent.is_some(), "the nested enum wins");

        // The nested declaration is reachable from outside by its path.
        compiled(&["namespace a;\nmessage M { enum R { Y } r: R, }\nstruct S { r: M.R }\n"]);
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
    fn consts_evaluate() {
        let schema = compiled(&[concat!(
            "namespace a;\n",
            "const A: i32 = 41;\n",
            "const B: i64 = A;\n",
            "const F: f32 = 1;\n",
            "const S: string = \"a\\nb\";\n",
            "const T: bool = true;\n",
        )]);
        assert_eq!(konst(&schema, "B").value, Value::Int(41));
        assert_eq!(konst(&schema, "F").value, Value::Float(1.0));
        assert_eq!(konst(&schema, "S").value, Value::Str("a\nb".into()));
        assert_eq!(konst(&schema, "T").value, Value::Bool(true));
    }

    #[test]
    fn consts_are_validated() {
        assert_eq!(
            messages(&["namespace a;\nconst C: u8 = 256;\n"]),
            ["`256` is out of range for `u8`"]
        );
        assert_eq!(
            messages(&["namespace a;\nconst C: i32 = \"hi\";\n"]),
            ["mismatched types: expected `i32`, found a string"]
        );
        assert_eq!(
            messages(&["namespace a;\nstruct S {}\nconst C: S = 1;\n"]),
            ["`S` cannot be the type of a constant"]
        );
        assert_eq!(
            messages(&["namespace a;\nconst C: vec<i32> = 1;\n"]),
            ["`vec<i32>` cannot be the type of a constant"]
        );
        assert_eq!(
            messages(&["namespace a;\nconst A: i32 = B;\nconst B: i32 = A;\n"]),
            ["constant `A` is defined in terms of itself"]
        );
        assert_eq!(
            messages(&["namespace a;\nenum E { X }\nenum F { Y }\nconst C: E = F.Y;\n"]),
            ["mismatched types: expected `E`, found a variant of enum `F`"]
        );
        // Constants are not types, and types are not values.
        assert_eq!(
            messages(&["namespace a;\nconst C: i32 = 1;\nstruct S { x: C }\n"]),
            ["`C` is not a type"]
        );
        assert_eq!(
            messages(&["namespace a;\nstruct S {}\nconst C: i32 = S;\n"]),
            ["`S` is a type, not a value"]
        );
    }

    #[test]
    fn unions_are_validated() {
        assert_eq!(
            messages(&["namespace a;\nunion U {}\n"]),
            ["union `U` needs at least one member"]
        );
        assert_eq!(
            messages(&["namespace a;\nunion U { a: i32? }\n"]),
            ["a union member cannot be optional"]
        );
    }

    #[test]
    fn messages_are_standalone() {
        assert_eq!(
            messages(&["namespace a;\nmessage M {}\nunion U { m: M }\n"]),
            ["`M` is a message; messages are standalone and cannot be used as a type"]
        );
        assert_eq!(
            messages(&["namespace a;\nmessage M {}\nstruct S { m: M }\n"]).len(),
            1
        );
        assert_eq!(
            messages(&["namespace a;\nmessage M {}\nmessage N { v: vec<M> }\n"]).len(),
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
            messages(&["namespace a;\nunion U { x: i32 }\nstruct S { u: U }\n"]),
            ["`U` is variable-size; a struct field must be fixed-size"]
        );
        assert_eq!(
            messages(&["namespace a;\nstruct S { x: i32? }\n"]),
            ["a struct field cannot be optional"]
        );
    }

    #[test]
    fn containers_need_fixed_size_items() {
        assert_eq!(
            messages(&["namespace a;\nmessage M { a: array<vec<i32>, 2> }\n"]),
            ["`vec<i32>` is variable-size; an array item must be fixed-size"]
        );
        assert_eq!(
            messages(&["namespace a;\nmessage M { s: set<string> }\n"]),
            ["`string` is variable-size; a set item must be fixed-size"]
        );
        assert_eq!(
            messages(&["namespace a;\nmessage M { m: map<string, i32> }\n"]),
            ["`string` is variable-size; a map key must be fixed-size"]
        );
        // Map values may be variable-size.
        compiled(&["namespace a;\nmessage M { m: map<i32, vec<i32>> }\n"]);
    }

    #[test]
    fn a_response_must_be_a_message() {
        assert_eq!(
            messages(&["namespace a;\nstruct S {}\nmessage M -> S {}\n"]),
            ["`S` is not a message"]
        );
        // Responses may be declared after the message referencing them.
        compiled(&["namespace a;\nmessage A -> B {}\nmessage B {}\n"]);
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
        compiled(&["namespace a;\nstruct S { x: i32 }\nmessage M { v: vec<S> }\n"]);
        compiled(&["namespace a;\nunion U { u: U, x: i32 }\n"]);
    }
}
