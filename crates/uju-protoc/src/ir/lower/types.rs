//! Types as written in fields, keys and targets, with their constraints.

use super::*;

impl<'a, 'src> Lowerer<'a, 'src> {
    pub(super) fn lower_type(
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
                Ok(id) => match self.resolver.types[id.0].item {
                    ast::Item::Message(_) => {
                        self.error(
                            source,
                            &ty.span,
                            format!(
                                "`{path}` is a message; messages are standalone and cannot be used as a type"
                            ),
                        );
                        TypeKind::Bool
                    }
                    ast::Item::Hub(_) => {
                        self.error(
                            source,
                            &ty.span,
                            "a hub kind is not a type; declare a struct for what you send",
                        );
                        TypeKind::Bool
                    }
                    ast::Item::Component(_) => {
                        self.error(
                            source,
                            &ty.span,
                            "a component is not a type; declare a struct for what you send",
                        );
                        TypeKind::Bool
                    }
                    ast::Item::Tag(_) => {
                        self.error(source, &ty.span, "a tag is not a type; it has no value");
                        TypeKind::Bool
                    }
                    ast::Item::Audience(_) => {
                        self.error(
                            source,
                            &ty.span,
                            "an audience is not a type; name its key's type instead",
                        );
                        TypeKind::Bool
                    }
                    _ => TypeKind::Named(id),
                },
                Err(diagnostic) => {
                    self.diagnostics.push(diagnostic);
                    TypeKind::Bool
                }
            },
        };
        let constraints = self.lower_constraints(source, ty, &kind);
        Type { kind, constraints }
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
    pub(super) fn bound(
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
}

/// Which constraints a type takes.
pub(super) enum Class {
    /// Integers, with the inclusive range of the type.
    Int(i128, i128),
    /// Floats, with the largest finite magnitude of the type.
    Float(f64),
    /// Strings, bytes, and variable-size collections.
    Length,
    None,
}

pub(super) fn constraint_class(kind: &TypeKind) -> Class {
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

pub(super) fn prim_type(prim: ast::Prim) -> TypeKind {
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
    use crate::ir::lower::tests::*;

    #[test]
    fn container_types_lower() {
        let schema = compiled(&[concat!(
            "namespace a\n",
            "struct Position { x: f32 }\n",
            "message(client -> sim) NonScalars {\n",
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
    fn collections_cannot_be_optional() {
        for ty in ["vec<i32>", "set<i32>", "map<i32, i32>", "array<i32, 2>"] {
            assert_eq!(
                messages(&[&format!(
                    "namespace a\nmessage(client -> sim) M {{ o: option<{ty}> }}\n"
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
            "message(client -> sim) M { s: option<string>, b: option<bytes>, p: option<P>, e: option<E> }\n",
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
            "message(client -> sim) M {\n",
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
            let src = format!("namespace a\nmessage(client -> sim) M {{ x: {ty} }}\n");
            assert_eq!(messages(&[&src]), [expected], "{ty}");
        }
        // User types take no constraints either.
        assert_eq!(
            messages(&["namespace a\nenum E { A }\nmessage(client -> sim) M { e: E[max=1] }\n"]),
            ["`E` takes no constraints"]
        );
    }

    #[test]
    fn options_only_wrap_a_field() {
        assert_eq!(
            messages(&["namespace a\nmessage(client -> sim) M { v: vec<option<i32>> }\n"]),
            ["`option<i32>` is nested; `option` is only allowed as a field's own type"]
        );
        assert_eq!(
            messages(&["namespace a\nmessage(client -> sim) M { o: option<option<i32>> }\n"]),
            ["`option<i32>` is nested; `option` is only allowed as a field's own type"]
        );
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
            let src = format!("{prelude}message(client -> sim) M {{ {field} }}\n");
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
}
