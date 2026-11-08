//! Attributes: which declarations take each, and the arguments of
//! `@replicate`, `@visibility` and `@halo`.

use super::*;

impl<'a, 'src> Lowerer<'a, 'src> {
    /// Check a declaration's attributes: each given once, known, and on a
    /// declaration it applies to. Their arguments are checked where the
    /// declaration is lowered.
    pub(super) fn check_attrs(
        &mut self,
        source: SourceId,
        attrs: &[ast::Attribute<'src>],
        keyword: &str,
    ) {
        let mut seen = HashSet::new();
        for attr in attrs {
            let (name, span) = &attr.name;
            if !seen.insert(*name) {
                self.error(source, span, format!("duplicate attribute `{name}`"));
                continue;
            }
            let message = match (*name, keyword) {
                ("replicate", "component") | ("visibility" | "halo", "component" | "tag") => {
                    continue;
                }
                ("replicate", "tag") => {
                    "a tag has no value to replicate; `@replicate` goes only on a component"
                        .to_string()
                }
                ("replicate", _) => "`@replicate` goes only on a component".to_string(),
                ("visibility" | "halo", _) => {
                    format!("`@{name}` goes only on a component or a tag")
                }
                _ => format!("unknown attribute `{name}`"),
            };
            self.error(source, span, message);
        }
    }

    /// The first attribute named `name`; later ones are reported as
    /// duplicates.
    fn attr<'i>(attrs: &'i [ast::Attribute<'src>], name: &str) -> Option<&'i ast::Attribute<'src>> {
        attrs.iter().find(|attr| attr.name.0 == name)
    }

    /// The single argument of `attr`, reporting any other count.
    fn single_arg<'i>(
        &mut self,
        source: SourceId,
        attr: &'i ast::Attribute<'src>,
        expected: &str,
    ) -> Option<&'i ast::AttributeArg<'src>> {
        match &attr.args[..] {
            [arg] => Some(arg),
            [] => {
                self.error(
                    source,
                    &attr.span,
                    format!("`@{}` takes one argument: {expected}", attr.name.0),
                );
                None
            }
            [_, extra, ..] => {
                self.error(
                    source,
                    extra.span(),
                    format!("`@{}` takes one argument: {expected}", attr.name.0),
                );
                None
            }
        }
    }

    /// `@replicate(replace | patch | ops(Union))`, `replace` when absent.
    /// The union resolves where the component is declared.
    pub(super) fn lower_replicate(
        &mut self,
        source: SourceId,
        scope: Option<TypeId>,
        attrs: &[ast::Attribute<'src>],
    ) -> Replicate {
        const EXPECTED: &str = "`replace`, `patch` or `ops(Union)`";
        let Some(attr) = Self::attr(attrs, "replicate") else {
            return Replicate::Replace;
        };
        let Some(arg) = self.single_arg(source, attr, EXPECTED) else {
            return Replicate::Replace;
        };
        match arg {
            ast::AttributeArg::Path(path) if is_word(path, "replace") => Replicate::Replace,
            ast::AttributeArg::Path(path) if is_word(path, "patch") => Replicate::Patch,
            ast::AttributeArg::Call { path, args, span } if is_word(path, "ops") => {
                let [ast::AttributeArg::Path(op)] = &args[..] else {
                    self.error(source, span, "`ops` takes one argument: the op union");
                    return Replicate::Replace;
                };
                match self.resolver.resolve(source, scope, op) {
                    Ok(id) if matches!(self.resolver.types[id.0].item, ast::Item::Union(_)) => {
                        Replicate::Ops(id)
                    }
                    Ok(_) => {
                        self.error(
                            source,
                            &op.span,
                            format!("`{op}` is not a union; the op type of `ops` must be a union"),
                        );
                        Replicate::Replace
                    }
                    Err(diagnostic) => {
                        self.diagnostics.push(diagnostic);
                        Replicate::Replace
                    }
                }
            }
            _ => {
                self.error(
                    source,
                    arg.span(),
                    format!("`{arg}` is not a replication mode; expected {EXPECTED}"),
                );
                Replicate::Replace
            }
        }
    }

    /// `@visibility(public | owner | none | Audience)`, `public` when absent.
    /// The audience resolves where the component or tag is declared.
    pub(super) fn lower_visibility(
        &mut self,
        source: SourceId,
        scope: Option<TypeId>,
        attrs: &[ast::Attribute<'src>],
    ) -> Visibility {
        const EXPECTED: &str = "`public`, `owner`, `none` or an audience";
        let Some(attr) = Self::attr(attrs, "visibility") else {
            return Visibility::Public;
        };
        let Some(arg) = self.single_arg(source, attr, EXPECTED) else {
            return Visibility::Public;
        };
        let ast::AttributeArg::Path(path) = arg else {
            self.error(
                source,
                arg.span(),
                format!("`{arg}` is not a visibility; expected {EXPECTED}"),
            );
            return Visibility::Public;
        };
        if is_word(path, "public") {
            return Visibility::Public;
        }
        if is_word(path, "owner") {
            return Visibility::Owner;
        }
        if is_word(path, "none") {
            return Visibility::None;
        }
        match self.resolver.resolve(source, scope, path) {
            Ok(id) if matches!(self.resolver.types[id.0].item, ast::Item::Audience(_)) => {
                Visibility::Audience(id)
            }
            Ok(_) => {
                self.error(
                    source,
                    &path.span,
                    format!("`{path}` is not an audience; expected {EXPECTED}"),
                );
                Visibility::Public
            }
            Err(diagnostic) => {
                self.diagnostics.push(diagnostic);
                Visibility::Public
            }
        }
    }

    /// `@halo(true | false)`; when absent, `true` unless no client sees
    /// the component, as a missing halo only fails once cells land on
    /// different shards.
    pub(super) fn lower_halo(
        &mut self,
        source: SourceId,
        attrs: &[ast::Attribute<'src>],
        visibility: Visibility,
    ) -> bool {
        let default = visibility != Visibility::None;
        let Some(attr) = Self::attr(attrs, "halo") else {
            return default;
        };
        let Some(arg) = self.single_arg(source, attr, "`true` or `false`") else {
            return default;
        };
        match arg {
            ast::AttributeArg::Literal((ast::Literal::Bool(value), _)) => *value,
            _ => {
                self.error(
                    source,
                    arg.span(),
                    format!("`{arg}` is not a bool; `@halo` takes `true` or `false`"),
                );
                default
            }
        }
    }
}

/// Whether `path` is the single word `word`, as `replace` in
/// `@replicate(replace)`.
fn is_word(path: &ast::Path<'_>, word: &str) -> bool {
    matches!(&path.segments[..], [(segment, _)] if *segment == word)
}

#[cfg(test)]
mod tests {
    use crate::compile;
    use crate::ir::lower::tests::*;

    #[test]
    fn attributes_go_only_where_they_apply() {
        let src = concat!(
            "namespace a\n",
            "@visibility(Faction)\n",
            "struct S {\n",
            "    @replicate(ops(Op))\n",
            "    enum E { A }\n",
            "    x: u8,\n",
            "}\n",
            "@large message(client -> sim) M {}\n",
            "@lock(Inventory, Gold) union U { A }\n",
            "@halo(false) type T = u8\n",
            "@cached hub(u64) H {}\n",
            "@timeout_ms(1000) const C: u8 = 1\n",
            "@replicate(patch) tag D\n",
            "@visibility(public) audience F: u8\n",
            "@lock(x) component P { x: u8 }\n",
        );
        let errors = compile(&[src]).expect_err("expected errors");
        assert_eq!(
            errors
                .iter()
                .map(|error| (&src[error.span.clone()], error.message.as_str()))
                .collect::<Vec<_>>(),
            [
                (
                    "visibility",
                    "`@visibility` goes only on a component or a tag"
                ),
                ("replicate", "`@replicate` goes only on a component"),
                ("large", "unknown attribute `large`"),
                ("lock", "unknown attribute `lock`"),
                ("halo", "`@halo` goes only on a component or a tag"),
                ("cached", "unknown attribute `cached`"),
                (
                    "replicate",
                    "a tag has no value to replicate; `@replicate` goes only on a component"
                ),
                (
                    "visibility",
                    "`@visibility` goes only on a component or a tag"
                ),
                ("lock", "unknown attribute `lock`"),
                // Consts lower after every type.
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
    fn component_attributes_are_validated() {
        let prelude = concat!(
            "namespace a\n",
            "audience Faction: u8\n",
            "union Op { A }\n",
            "struct S {}\n",
        );
        let cases = [
            (
                "@replicate",
                "@replicate",
                "`@replicate` takes one argument: `replace`, `patch` or `ops(Union)`",
            ),
            (
                "@replicate(patch, replace)",
                "replace",
                "`@replicate` takes one argument: `replace`, `patch` or `ops(Union)`",
            ),
            (
                "@replicate(diff)",
                "diff",
                "`diff` is not a replication mode; expected `replace`, `patch` or `ops(Union)`",
            ),
            (
                "@replicate(\"patch\")",
                "\"patch\"",
                "`\"patch\"` is not a replication mode; expected `replace`, `patch` or `ops(Union)`",
            ),
            (
                "@replicate(a.patch)",
                "a.patch",
                "`a.patch` is not a replication mode; expected `replace`, `patch` or `ops(Union)`",
            ),
            (
                "@replicate(patch(Op))",
                "patch(Op)",
                "`patch(Op)` is not a replication mode; expected `replace`, `patch` or `ops(Union)`",
            ),
            (
                "@replicate(ops(Op, Op))",
                "ops(Op, Op)",
                "`ops` takes one argument: the op union",
            ),
            (
                "@replicate(ops(1))",
                "ops(1)",
                "`ops` takes one argument: the op union",
            ),
            (
                "@replicate(ops(S))",
                "S",
                "`S` is not a union; the op type of `ops` must be a union",
            ),
            (
                "@replicate(ops(Faction))",
                "Faction",
                "`Faction` is not a union; the op type of `ops` must be a union",
            ),
            (
                "@replicate(ops(Missing))",
                "Missing",
                "cannot find `Missing` in this scope",
            ),
            (
                "@visibility(team)",
                "team",
                "cannot find `team` in this scope",
            ),
            (
                "@visibility(S)",
                "S",
                "`S` is not an audience; expected `public`, `owner`, `none` or an audience",
            ),
            (
                "@visibility(Op)",
                "Op",
                "`Op` is not an audience; expected `public`, `owner`, `none` or an audience",
            ),
            (
                "@visibility(\"owner\")",
                "\"owner\"",
                "`\"owner\"` is not a visibility; expected `public`, `owner`, `none` or an audience",
            ),
            (
                "@visibility(owner, none)",
                "none",
                "`@visibility` takes one argument: `public`, `owner`, `none` or an audience",
            ),
            (
                "@halo",
                "@halo",
                "`@halo` takes one argument: `true` or `false`",
            ),
            (
                "@halo(1)",
                "1",
                "`1` is not a bool; `@halo` takes `true` or `false`",
            ),
            (
                "@halo(yes)",
                "yes",
                "`yes` is not a bool; `@halo` takes `true` or `false`",
            ),
        ];
        for (attr, written, expected) in cases {
            let src = format!("{prelude}{attr}\ncomponent C {{ x: u8 }}\n");
            assert_eq!(
                spanned(&src),
                [(written.to_string(), expected.to_string())],
                "{attr}"
            );
        }
        // Tags check `@visibility` and `@halo` the same way.
        assert_eq!(
            spanned(&format!("{prelude}@visibility(S) @halo(0) tag T\n")),
            [
                (
                    "S".to_string(),
                    "`S` is not an audience; expected `public`, `owner`, `none` or an audience"
                        .to_string()
                ),
                (
                    "0".to_string(),
                    "`0` is not a bool; `@halo` takes `true` or `false`".to_string()
                ),
            ]
        );
        // Attribute paths resolve where the component is declared, outside
        // its body.
        assert_eq!(
            messages(&[
                "namespace a\n@replicate(ops(Op))\ncomponent C { x: u8, union Op { A } }\n"
            ]),
            ["cannot find `Op` in this scope"]
        );
        // And across namespaces, through `use`.
        compiled(&[
            "namespace b\naudience Faction: u8\nunion Op { A }\n",
            "namespace a\nuse b\n@replicate(ops(b.Op)) @visibility(b.Faction)\ncomponent C { x: u8 }\n",
        ]);
    }
}
