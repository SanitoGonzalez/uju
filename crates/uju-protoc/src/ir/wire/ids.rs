//! Ids: messages per role pair, hub kinds, components and tags, and
//! audiences.

use std::collections::BTreeMap;

use super::*;
use crate::ir::{DefKind, Link};

/// The id space of one role pair: the messages linking `from` to `to`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Space {
    pub from: Role,
    pub to: Role,
    /// In id order from 1, which is qualified name order.
    pub messages: Vec<TypeId>,
}

/// The ids of messages, hub kinds, components and tags, and audiences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Ids {
    /// Indexed by [`TypeId`]: a message's id on each of its links, in
    /// link order. Empty for everything but messages.
    pub(super) messages: Vec<Vec<u16>>,
    /// The role pairs some message links, in canonical role order.
    pub(super) spaces: Vec<Space>,
    pub(super) hubs: Dense,
    /// Components and tags share one space. Those clients see come first,
    /// so their ids never shift when a server-only one changes, and the
    /// rest follow, so server links carry both without colliding.
    pub(super) components: Dense,
    /// How many of `components` clients see.
    pub(super) client_components: usize,
    pub(super) audiences: Dense,
}

impl Ids {
    pub(super) fn new(schema: &Schema, errors: &mut Vec<Error>) -> Ids {
        let len = schema.defs.len();
        let (messages, spaces) = messages(schema, errors);

        let by_name = |filter: fn(&DefKind) -> bool| {
            let mut defs: Vec<(String, TypeId)> = schema
                .defs
                .iter()
                .enumerate()
                .filter(|(_, def)| filter(&def.kind))
                .map(|(index, _)| (schema.qualified(TypeId(index)), TypeId(index)))
                .collect();
            // Qualified names are unique.
            defs.sort_by(|left, right| left.0.cmp(&right.0));
            defs.into_iter().map(|(_, id)| id).collect::<Vec<_>>()
        };
        let hubs = by_name(|kind| matches!(kind, DefKind::Hub(_)));
        let audiences = by_name(|kind| matches!(kind, DefKind::Audience(_)));
        let (client, server): (Vec<TypeId>, Vec<TypeId>) =
            by_name(|kind| matches!(kind, DefKind::Component(_) | DefKind::Tag(_)))
                .into_iter()
                .partition(|id| match &schema.def(*id).kind {
                    DefKind::Component(item) => item.visibility.reaches_clients(),
                    DefKind::Tag(item) => item.visibility.reaches_clients(),
                    _ => unreachable!("only components and tags are numbered here"),
                });
        let client_components = client.len();
        let components = client.into_iter().chain(server).collect();

        Ids {
            messages,
            spaces,
            hubs: Dense::new(len, hubs, "hub kinds", errors),
            components: Dense::new(len, components, "components and tags", errors),
            client_components,
            audiences: Dense::new(len, audiences, "audiences", errors),
        }
    }
}

/// Ids dense from 1 over one kind of declaration, as message ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Dense {
    /// Indexed by [`TypeId`]; `Some` for the declarations numbered.
    ids: Vec<Option<u16>>,
    /// In id order.
    pub(super) order: Vec<TypeId>,
}

impl Dense {
    /// Numbers `order` from 1, reporting each past `u16::MAX` as "more
    /// than 65535 {what}".
    fn new(len: usize, order: Vec<TypeId>, what: &str, errors: &mut Vec<Error>) -> Dense {
        let mut ids = vec![None; len];
        for (index, id) in order.iter().enumerate() {
            ids[id.0] = u16::try_from(index + 1).ok();
            if ids[id.0].is_none() {
                errors.push(Error {
                    def: *id,
                    variant: None,
                    field: None,
                    message: format!("more than {} {what}", u16::MAX),
                });
            }
        }
        Dense { ids, order }
    }

    pub(super) fn id(&self, id: TypeId) -> Option<u16> {
        self.ids[id.0]
    }
}

/// Each message's id on each of its links, and the messages of each role
/// pair in id order.
fn messages(schema: &Schema, errors: &mut Vec<Error>) -> (Vec<Vec<u16>>, Vec<Space>) {
    let mut messages: Vec<(String, TypeId, &[Link])> = schema
        .defs
        .iter()
        .enumerate()
        .filter_map(|(index, def)| match &def.kind {
            DefKind::Message(item) => Some((
                schema.qualified(TypeId(index)),
                TypeId(index),
                &item.links[..],
            )),
            _ => None,
        })
        .collect();
    // Qualified names are unique.
    messages.sort_by(|left, right| left.0.cmp(&right.0));
    let mut ids = vec![Vec::new(); schema.defs.len()];
    let mut spaces: BTreeMap<(Role, Role), Vec<TypeId>> = BTreeMap::new();
    for (_, id, links) in messages {
        for link in links {
            let space = spaces.entry((link.from, link.to)).or_default();
            space.push(id);
            // From 1: a zeroed id is no message.
            let index = u16::try_from(space.len()).unwrap_or_else(|_| {
                errors.push(Error {
                    def: id,
                    variant: None,
                    field: None,
                    message: format!(
                        "more than {} `{} -> {}` messages",
                        u16::MAX,
                        link.from,
                        link.to
                    ),
                });
                0
            });
            ids[id.0].push(index);
        }
    }
    let spaces = spaces
        .into_iter()
        .map(|((from, to), messages)| Space { from, to, messages })
        .collect();
    (ids, spaces)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::wire::tests::*;

    #[test]
    fn messages_take_dense_ids_per_role_pair() {
        let (schema, wire) = compiled(&[concat!(
            "namespace a\n",
            "hub(u64) P {}\n",
            "message(client -> sim) C {}\n",
            "message(client <-> sim) B {}\n",
            "message(sim | hub -> client) D {}\n",
            "message(client -> sim | hub<P>) A {}\n",
            "message(sim <-> sim) E {}\n",
        )]);
        let spaces: Vec<(Role, Role, Vec<String>)> = wire
            .spaces()
            .iter()
            .map(|space| {
                let names = space.messages.iter().map(|id| schema.qualified(*id));
                (space.from, space.to, names.collect())
            })
            .collect();
        assert_eq!(
            spaces,
            [
                (
                    Role::Client,
                    Role::Sim,
                    vec!["a.A".into(), "a.B".into(), "a.C".into()]
                ),
                (Role::Client, Role::Hub, vec!["a.A".into()]),
                (Role::Sim, Role::Client, vec!["a.B".into(), "a.D".into()]),
                (Role::Sim, Role::Sim, vec!["a.E".into()]),
                (Role::Hub, Role::Client, vec!["a.D".into()]),
            ]
        );
        let b = id(&schema, "B");
        assert_eq!(wire.ids(b), [2, 1], "in link order");
        assert_eq!(wire.message_id(b, Role::Client, Role::Sim), Some(2));
        assert_eq!(wire.message_id(b, Role::Sim, Role::Client), Some(1));
        assert_eq!(wire.message_id(b, Role::Hub, Role::Client), None);
        assert_eq!(wire.ids(id(&schema, "A")), [1, 1], "no message is 0");
        assert_eq!(wire.ids(id(&schema, "E")), [1], "`sim <-> sim` is one pair");
        assert!(wire.space(Role::Entry, Role::Client).is_empty());
        assert!(wire.ids(id(&schema, "P")).is_empty());
    }

    #[test]
    fn hub_kinds_take_dense_ids_by_qualified_name() {
        let (schema, wire) = compiled(&[
            "namespace b\nhub(u64) Guild {}\nhub(u32) Bank {}\n",
            "namespace a\nhub(u64) Party {}\nstruct S {}\n",
        ]);
        let names: Vec<String> = wire.hubs().iter().map(|id| schema.qualified(*id)).collect();
        assert_eq!(names, ["a.Party", "b.Bank", "b.Guild"]);
        assert_eq!(wire.hub_id(id(&schema, "Party")), Some(1), "no kind is 0");
        assert_eq!(wire.hub_id(id(&schema, "Bank")), Some(2));
        assert_eq!(wire.hub_id(id(&schema, "Guild")), Some(3));
        assert_eq!(wire.hub_id(id(&schema, "S")), None);

        let (_, wire) = compiled(&["namespace a\nstruct S {}\n"]);
        assert!(wire.hubs().is_empty());
    }

    #[test]
    fn components_and_tags_share_ids_with_client_ones_first() {
        let (schema, wire) = compiled(&[
            concat!(
                "namespace b\n",
                "audience Stealth: u8\n",
                "@visibility(none) component Cooldowns { x: u8 }\n",
                "@visibility(Stealth) tag Hidden\n",
                "@visibility(owner) component Bag { x: u8 }\n",
            ),
            concat!(
                "namespace a\n",
                "audience Faction: u32\n",
                "@visibility(none) tag Busy\n",
                "tag Dead\n",
                "component Health { hp: u32 }\n",
                "struct S {}\n",
            ),
        ]);
        let names = |ids: &[TypeId]| -> Vec<String> {
            ids.iter().map(|id| schema.qualified(*id)).collect()
        };
        assert_eq!(
            names(wire.components()),
            [
                "a.Dead",
                "a.Health",
                "b.Bag",
                "b.Hidden",
                "a.Busy",
                "b.Cooldowns"
            ]
        );
        assert_eq!(
            names(wire.client_components()),
            ["a.Dead", "a.Health", "b.Bag", "b.Hidden"]
        );
        for (index, id) in wire.components().iter().enumerate() {
            assert_eq!(wire.component_id(*id), Some(index as u16 + 1), "no id is 0");
        }
        assert_eq!(wire.component_id(id(&schema, "S")), None);
        assert_eq!(wire.component_id(id(&schema, "Faction")), None);

        assert_eq!(names(wire.audiences()), ["a.Faction", "b.Stealth"]);
        assert_eq!(wire.audience_id(id(&schema, "Faction")), Some(1));
        assert_eq!(wire.audience_id(id(&schema, "Stealth")), Some(2));
        assert_eq!(wire.audience_id(id(&schema, "Dead")), None);

        // A server-only component never shifts a client one's id.
        let (schema, wire) = compiled(&["namespace a\n@visibility(none) tag A\ntag B\n"]);
        assert_eq!(wire.component_id(id(&schema, "B")), Some(1));
        assert_eq!(wire.component_id(id(&schema, "A")), Some(2));

        let (_, wire) = compiled(&["namespace a\nstruct S {}\n"]);
        assert!(wire.components().is_empty());
        assert!(wire.audiences().is_empty());
    }
}
