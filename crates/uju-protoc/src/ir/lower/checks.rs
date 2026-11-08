//! Checks that need every declaration lowered: fixed-size types, wire
//! sizes, map and hub keys, and recursion.

use super::*;

/// A type that must be fixed-size, checked once every declaration is
/// lowered.
pub(super) struct FixedCheck {
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
    pub(super) fn require_fixed(
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

    pub(super) fn check_fixed(&mut self, schema: &Schema) {
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
    pub(super) fn check_wire(&mut self, schema: &Schema) {
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
    pub(super) fn check_key(
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
                let body = match site.item {
                    ast::Item::Struct(ast::Struct { body, .. }) => body,
                    ast::Item::Type(item) => {
                        visiting.push(id);
                        let path = self.holds_float(site.source, Some(id), &item.target, visiting);
                        visiting.pop();
                        return path;
                    }
                    _ => return None,
                };
                visiting.push(id);
                body.fields.iter().find_map(|field| {
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

    /// Report recursive types, through any field: inline, optional, or
    /// inside a collection. Recursion would let a message nest as deep as
    /// its size allows, and every decoder would have to bound its stack.
    pub(super) fn check_cycles(&mut self, defs: &[Def]) {
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
            DefKind::Struct(Struct { fields })
            | DefKind::Message(Message { fields, .. })
            | DefKind::Hub(Hub { fields, .. })
            | DefKind::Component(Component { fields, .. }) => {
                for field in fields {
                    refs(&field.ty, &mut targets);
                }
            }
            DefKind::Audience(_) | DefKind::Tag(_) => {}
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

#[cfg(test)]
mod tests {
    use crate::compile;
    use crate::ir::lower::tests::*;

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
            let src = format!("{prelude}message(client -> sim) M {{ {field} }}\n");
            assert_eq!(messages(&[&src]), [expected], "{field}");
        }

        // A fixed length makes strings fixed-size, and `option<bool>` stays
        // in the bitmap.
        compiled(&[&format!(
            "{prelude}message(client -> sim) M {{ a: array<Flags, 2>, s: set<Code>, m: map<bytes[min_length=4, max_length=4], u8> }}\n"
        )]);
    }

    #[test]
    fn lengths_fit_a_message() {
        assert_eq!(
            messages(&["namespace a\nmessage(client -> sim) M { s: string[max_length=65536] }\n"]),
            ["`max_length` must be at most 65535, the size of a message"]
        );
        compiled(&["namespace a\nmessage(client -> sim) M { s: vec<u8>[max_length=65535] }\n"]);
    }

    #[test]
    fn containers_need_fixed_size_items() {
        assert_eq!(
            messages(&["namespace a\nmessage(client -> sim) M { a: array<vec<i32>, 2> }\n"]),
            ["`vec<i32>` is variable-size; an array item must be fixed-size"]
        );
        assert_eq!(
            messages(&["namespace a\nmessage(client -> sim) M { s: set<string> }\n"]),
            ["`string` is variable-size; a set item must be fixed-size"]
        );
        assert_eq!(
            messages(&["namespace a\nmessage(client -> sim) M { m: map<string, i32> }\n"]),
            ["`string` is variable-size; a map key must be fixed-size"]
        );
        // Map values may be variable-size.
        compiled(&["namespace a\nmessage(client -> sim) M { m: map<i32, vec<i32>> }\n"]);
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
            let src = format!("{prelude}message(client -> sim) M {{ {field} }}\n");
            assert_eq!(messages(&[&src]), [expected], "{field}");
        }

        // Floats are fine as map values, and float-free structs as keys.
        compiled(&[concat!(
            "namespace a\n",
            "struct Cell { x: i32, y: i32 }\n",
            "message(client -> sim) M { m: map<Cell, f32[nan]>, s: set<Cell> }\n",
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
    fn hub_keys_are_fixed_size_and_hold_no_float() {
        compiled(&[concat!(
            "namespace a\n",
            "enum E: u8 { A }\n",
            "type Id = u64\n",
            "struct Pair { a: u32, b: E }\n",
            "hub(u8) A {}\n",
            "hub(E) B {}\n",
            "hub(Id) C {}\n",
            "hub(Pair) D {}\n",
            "hub(uuid) F {}\n",
            "hub(entity) G {}\n",
            "hub(array<u8, 4>) H {}\n",
            "hub(string[min_length=4, max_length=4]) I {}\n",
        )]);

        let prelude = concat!(
            "namespace a\n",
            "struct Position { x: f32, y: f32 }\n",
            "message(client -> sim) M {}\n",
        );
        let cases = [
            ("f32", "`f32` holds a float; a hub key cannot hold a float"),
            (
                "Position",
                "`Position` holds a float through field `x`; a hub key cannot hold a float",
            ),
            (
                "string",
                "`string` is variable-size; a hub key must be fixed-size",
            ),
            (
                "vec<u8>",
                "`vec<u8>` is variable-size; a hub key must be fixed-size",
            ),
            ("option<u64>", "a hub key cannot be optional"),
            (
                "M",
                "`M` is a message; messages are standalone and cannot be used as a type",
            ),
            ("Missing", "cannot find `Missing` in this scope"),
            (
                "K",
                "a hub kind is not a type; declare a struct for what you send",
            ),
            (
                "array<u64, 9000>",
                "`array<u64, 9000>` takes 72000 bytes; a message holds at most 65535",
            ),
        ];
        for (key, expected) in cases {
            let src = format!("{prelude}hub({key}) K {{}}\n");
            assert_eq!(messages(&[&src]), [expected], "{key}");
        }

        // The key is resolved where the kind is declared, outside its body.
        assert_eq!(
            messages(&["namespace a\nhub(Id) K { type Id = u64 }\n"]),
            ["cannot find `Id` in this scope"]
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

        compiled(&["namespace a\nstruct S { x: i32 }\nmessage(client -> sim) M { v: vec<S> }\n"]);
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

    #[test]
    fn audience_keys_are_fixed_size_and_hold_no_float() {
        compiled(&[concat!(
            "namespace a\n",
            "enum E: u8 { A }\n",
            "type Id = u64\n",
            "struct Pair { a: u32, b: E }\n",
            "audience A: u8\n",
            "audience B: E\n",
            "audience C: Id\n",
            "audience D: Pair\n",
            "audience G: array<u8, 4>\n",
        )]);
        let prelude = "namespace a\nstruct Position { x: f32, y: f32 }\n";
        let cases = [
            (
                "f32",
                "`f32` holds a float; an audience key cannot hold a float",
            ),
            (
                "Position",
                "`Position` holds a float through field `x`; an audience key cannot hold a float",
            ),
            (
                "string",
                "`string` is variable-size; an audience key must be fixed-size",
            ),
            ("option<u64>", "an audience key cannot be optional"),
            (
                "array<u64, 9000>",
                "`array<u64, 9000>` takes 72000 bytes; a message holds at most 65535",
            ),
        ];
        for (key, expected) in cases {
            let src = format!("{prelude}audience K: {key}\n");
            assert_eq!(messages(&[&src]), [expected], "{key}");
        }
    }
}
