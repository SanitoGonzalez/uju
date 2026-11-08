//! Message endpoints: roles, hub kinds, and the links between them.

use super::*;

/// One side of a message's endpoints, with its hub endpoints merged into
/// one [`Role::Hub`].
struct Side {
    /// Each role once.
    roles: Vec<Role>,
    /// Where bare `hub` is written, if it is.
    bare_hub: Option<Span>,
    /// The kinds of `hub<K>`, each once, where they are written.
    kinds: Vec<(TypeId, Span)>,
}

impl<'a, 'src> Lowerer<'a, 'src> {
    /// Every role pair of the product of `endpoints`' sides, and of the
    /// reverse product too with `<->`, each once. Sending kinds are sorted
    /// once the schema is built, since that needs qualified names.
    pub(super) fn lower_links(
        &mut self,
        source: SourceId,
        id: TypeId,
        endpoints: &ast::Endpoints<'src>,
    ) -> Vec<Link> {
        // Endpoints are written outside the message's body.
        let scope = self.resolver.types[id.0].parent;
        let from = self.lower_side(source, scope, &endpoints.from);
        let to = self.lower_side(source, scope, &endpoints.to);

        let mut receivers = vec![&to];
        if endpoints.both {
            receivers.push(&from);
        }
        let mut kind = None;
        for side in receivers {
            if let Some(span) = &side.bare_hub {
                self.error(
                    source,
                    span,
                    "nothing could route a message to any hub; name the kind, as `hub<Party>`",
                );
            }
            for (other, span) in &side.kinds {
                match kind {
                    None => kind = Some(*other),
                    Some(kind) if kind == *other => {}
                    Some(_) => {
                        self.error(source, span, "a message to a hub names exactly one kind")
                    }
                }
            }
        }

        // Both rules are symmetric, so the reverse product breaks none
        // the product does not.
        let arrow = if endpoints.both { "<->" } else { "->" };
        let mut reported = HashSet::new();
        for a in &from.roles {
            for b in &to.roles {
                let message = match (*a, *b) {
                    (Role::Client, Role::Client) => "clients reach each other only through servers",
                    (Role::Edge, other) | (other, Role::Edge) if other != Role::Client => {
                        "edges run no gameplay, so `edge` pairs only with `client`"
                    }
                    _ => continue,
                };
                if reported.insert((*a, *b)) {
                    self.error(
                        source,
                        &endpoints.span,
                        format!("`{a} {arrow} {b}` is not a link; {message}"),
                    );
                }
            }
        }

        let mut links = BTreeMap::new();
        let mut pairs = vec![(&from, &to)];
        if endpoints.both {
            pairs.push((&to, &from));
        }
        for (senders, receivers) in pairs {
            for a in &senders.roles {
                for b in &receivers.roles {
                    links.entry((*a, *b)).or_insert_with(|| Link {
                        from: *a,
                        to: *b,
                        from_kinds: (*a == Role::Hub && !senders.kinds.is_empty())
                            .then(|| senders.kinds.iter().map(|(kind, _)| *kind).collect()),
                        to_kind: (*b == Role::Hub)
                            .then(|| receivers.kinds.first().map(|(kind, _)| *kind))
                            .flatten(),
                    });
                }
            }
        }
        links.into_values().collect()
    }

    /// One side of a message's endpoints, reporting a role listed twice and
    /// `hub` beside `hub<K>`.
    fn lower_side(
        &mut self,
        source: SourceId,
        scope: Option<TypeId>,
        endpoints: &[ast::Endpoint<'src>],
    ) -> Side {
        let mut side = Side {
            roles: Vec::new(),
            bare_hub: None,
            kinds: Vec::new(),
        };
        for endpoint in endpoints {
            let span = &endpoint.span;
            let repeated = match &endpoint.kind {
                None if endpoint.role == Role::Hub => {
                    let repeated = side.bare_hub.is_some();
                    if !repeated
                        && !side.kinds.is_empty()
                        && let Some(kind) = endpoints.iter().find(|other| other.kind.is_some())
                    {
                        self.error(
                            source,
                            span,
                            format!("`hub` beside `{kind}`; bare `hub` already means any kind"),
                        );
                    }
                    side.bare_hub = Some(span.clone());
                    repeated
                }
                None => side.roles.contains(&endpoint.role),
                Some(path) => {
                    let Some(kind) = self.hub_kind(source, scope, path) else {
                        continue;
                    };
                    let repeated = side.kinds.iter().any(|(other, _)| *other == kind);
                    if !repeated && side.bare_hub.is_some() {
                        self.error(
                            source,
                            span,
                            format!("`{endpoint}` beside `hub`; bare `hub` already means any kind"),
                        );
                    }
                    if !repeated {
                        side.kinds.push((kind, span.clone()));
                    }
                    repeated
                }
            };
            if repeated {
                self.error(
                    source,
                    span,
                    format!("`{endpoint}` is listed twice on one side"),
                );
            } else if !side.roles.contains(&endpoint.role) {
                side.roles.push(endpoint.role);
            }
        }
        side
    }

    /// The hub kind `path` names, reporting anything else.
    fn hub_kind(
        &mut self,
        source: SourceId,
        scope: Option<TypeId>,
        path: &ast::Path<'src>,
    ) -> Option<TypeId> {
        match self.resolver.resolve(source, scope, path) {
            Ok(id) if matches!(self.resolver.types[id.0].item, ast::Item::Hub(_)) => Some(id),
            Ok(_) => {
                self.error(source, &path.span, format!("`{path}` is not a hub kind"));
                None
            }
            Err(diagnostic) => {
                self.diagnostics.push(diagnostic);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::lower::tests::*;

    #[test]
    fn messages_carry_their_links() {
        let schema = compiled(&[EXAMPLE1, EXAMPLE2]);
        let links = |name: &str| -> Vec<String> {
            let (_, def) = def(&schema, name);
            let DefKind::Message(message) = &def.kind else {
                panic!("expected a message");
            };
            message
                .links
                .iter()
                .map(|link| {
                    let kinds = |kinds: Vec<TypeId>| -> String {
                        let names: Vec<String> =
                            kinds.iter().map(|id| schema.qualified(*id)).collect();
                        format!("<{}>", names.join(" | "))
                    };
                    format!(
                        "{}{} -> {}{}",
                        link.from,
                        link.from_kinds.clone().map(kinds).unwrap_or_default(),
                        link.to,
                        link.to_kind.map(|id| kinds(vec![id])).unwrap_or_default(),
                    )
                })
                .collect()
        };
        assert_eq!(
            links("Ping"),
            [
                "client -> entry",
                "client -> edge",
                "entry -> client",
                "edge -> client"
            ]
        );
        assert_eq!(links("Party"), ["sim -> sim"], "deduplicated");
        assert_eq!(links("MyResponse"), ["sim -> client"]);
        assert_eq!(links("Status"), ["sim -> client", "hub -> client"]);
        assert_eq!(
            links("Deposit"),
            [
                "client -> hub<foo.bar.GuildBank>",
                "sim -> hub<foo.bar.GuildBank>"
            ]
        );
        assert_eq!(
            links("GuildNotice"),
            ["hub<foo.bar.Guild | foo.bar.GuildBank> -> client"]
        );
        // Resolved through `use foo.bar`.
        assert_eq!(links("GuildExp"), ["sim -> hub<foo.bar.Guild>"]);

        let (response_id, _) = def(&schema, "MyResponse");
        let (result_id, result) = def(&schema, "Result");
        assert_eq!(result.parent, Some(response_id));
        assert_eq!(schema.path(result_id), ["MyResponse", "Result"]);
    }

    #[test]
    fn sending_kinds_sort_by_qualified_name() {
        let schema = compiled(&[
            "namespace b\nhub(u8) A {}\n",
            "namespace a\nuse b\nhub(u8) Z {}\nmessage(hub<Z> | hub<b.A> -> sim) M {}\n",
        ]);
        let (_, def) = def(&schema, "M");
        let DefKind::Message(message) = &def.kind else {
            panic!("expected a message");
        };
        let kinds: Vec<String> = message.links[0]
            .from_kinds
            .iter()
            .flatten()
            .map(|id| schema.qualified(*id))
            .collect();
        assert_eq!(kinds, ["a.Z", "b.A"]);
    }

    #[test]
    fn endpoints_are_validated() {
        let prelude = "namespace a\nhub(u8) P {}\nhub(u8) Q {}\nstruct S {}\n";
        for (endpoints, expected) in [
            ("sim | sim -> client", "`sim` is listed twice on one side"),
            (
                "sim -> hub<P> | hub<P>",
                "`hub<P>` is listed twice on one side",
            ),
            ("hub | hub -> client", "`hub` is listed twice on one side"),
            (
                "hub<P> | hub -> client",
                "`hub` beside `hub<P>`; bare `hub` already means any kind",
            ),
            (
                "hub | hub<P> -> client",
                "`hub<P>` beside `hub`; bare `hub` already means any kind",
            ),
            (
                "client -> client",
                "`client -> client` is not a link; clients reach each other only through servers",
            ),
            (
                "client <-> sim | client",
                "`client <-> client` is not a link; clients reach each other only through servers",
            ),
            (
                "edge -> sim",
                "`edge -> sim` is not a link; edges run no gameplay, so `edge` pairs only with `client`",
            ),
            (
                "sim -> edge",
                "`sim -> edge` is not a link; edges run no gameplay, so `edge` pairs only with `client`",
            ),
            (
                "entry <-> edge",
                "`entry <-> edge` is not a link; edges run no gameplay, so `edge` pairs only with `client`",
            ),
            (
                "edge -> edge",
                "`edge -> edge` is not a link; edges run no gameplay, so `edge` pairs only with `client`",
            ),
            (
                "edge -> hub<P>",
                "`edge -> hub` is not a link; edges run no gameplay, so `edge` pairs only with `client`",
            ),
            (
                "sim -> hub",
                "nothing could route a message to any hub; name the kind, as `hub<Party>`",
            ),
            (
                "hub <-> sim",
                "nothing could route a message to any hub; name the kind, as `hub<Party>`",
            ),
            (
                "sim -> hub<P> | hub<Q>",
                "a message to a hub names exactly one kind",
            ),
            (
                "hub<P> <-> hub<Q>",
                "a message to a hub names exactly one kind",
            ),
            (
                "hub<P> | hub<Q> <-> client",
                "a message to a hub names exactly one kind",
            ),
            ("sim -> hub<S>", "`S` is not a hub kind"),
            ("sim -> hub<R>", "cannot find `R` in this scope"),
        ] {
            let src = format!("{prelude}message({endpoints}) M {{}}\n");
            assert_eq!(messages(&[&src]), [expected], "{endpoints}");
        }
        // Each pair is reported once, whichever way it goes.
        assert_eq!(
            messages(&["namespace a\nmessage(edge | sim <-> sim | edge) M {}\n"]),
            [
                "`edge <-> sim` is not a link; edges run no gameplay, so `edge` pairs only with `client`",
                "`edge <-> edge` is not a link; edges run no gameplay, so `edge` pairs only with `client`",
                "`sim <-> edge` is not a link; edges run no gameplay, so `edge` pairs only with `client`",
            ]
        );
        // The same kind on both sides is one kind.
        compiled(&[&format!(
            "{prelude}message(hub<P> | sim <-> hub<P>) M {{}}\n"
        )]);
        // Kinds resolve from the message's scope, as types do.
        compiled(&["namespace a\nhub(u8) P {}\nstruct O { message(sim -> hub<P>) M {} }\n"]);
    }
}
