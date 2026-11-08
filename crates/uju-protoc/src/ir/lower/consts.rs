//! Consts: their types and values.

use super::*;
use crate::ir::lower::types::{Class, constraint_class, prim_type};

impl<'a, 'src> Lowerer<'a, 'src> {
    /// Check a const's type, which must be a number, `bool`, `string`,
    /// `duration`, or `timestamp` without constraints, and its value against
    /// the type.
    pub(super) fn lower_const(&mut self, site: &ConstSite<'a, 'src>) -> Const {
        let source = site.source;
        let item = site.item;
        self.check_attrs(source, &item.attrs, "const");
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
}

/// The longest `duration` a const can take, in microseconds: 10000 Julian
/// years, or 100 centuries of 36525 days.
const MAX_DURATION_MICROS: u64 = 100 * 36_525 * 86_400 * 1_000_000;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::lower::tests::*;

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
            "message(client -> sim) M {\n",
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
            messages(&["namespace a\nmessage(client -> sim) M { const A: u8 = 1 enum A { X } }\n"]),
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
            messages(&[
                "namespace a\nstruct R {}\nmessage(client -> sim) M { const R: u8 = 1 r: R }\n"
            ]),
            ["`R` is a const, not a type"]
        );
    }
}
