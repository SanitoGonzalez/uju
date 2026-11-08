//! Fingerprints: the canonical text of what peers must agree on, and its
//! hash.

use std::fmt::Write as _;

use super::*;
use crate::ir::{
    ConstId, ConstValue, DefKind, Field, Link, Replicate, TypeKind, VariantData, Visibility,
};

/// What a fingerprint covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Scope {
    Client,
    Internal,
}

/// Every declaration in `scope` as written in source, sorted by qualified
/// name, so the order sources are compiled in does not matter. Field order
/// does: it is the layout. Consts follow the types, sorted the same way.
///
/// A message spells its links normalized: its pairs in canonical role
/// order, comma-separated, with kinds qualified and sending kinds sorted,
/// as `message(client -> hub<a.Party>, hub<a.Guild> | hub<a.Party> -> client)`.
/// For clients, a message lists only the links with a client end, and a hub
/// kind only its key and name, as `hub(u64) a.Party`.
///
/// Components and tags spell every attribute, defaults included, as
/// `@replicate(ops(a.Op)) @visibility(public) @halo(true) component a.C { .. }`
/// and `@visibility(a.Stealth) @halo(true) tag a.Hidden`. For clients,
/// `@halo` is left out. An audience is `audience a.Faction: a.FactionId`.
pub(super) fn canonical(schema: &Schema, scope: Scope) -> String {
    let listed = match scope {
        Scope::Internal => vec![true; schema.defs.len()],
        Scope::Client => client_listed(schema),
    };
    let mut defs: Vec<(String, TypeId)> = listed
        .iter()
        .enumerate()
        .filter(|(_, listed)| **listed)
        .map(|(index, _)| (schema.qualified(TypeId(index)), TypeId(index)))
        .collect();
    defs.sort_by(|left, right| left.0.cmp(&right.0));

    let fields = |out: &mut String, fields: &[Field]| {
        out.push_str(" {");
        for field in fields {
            let ty = schema.type_name(&field.ty);
            if field.optional {
                let _ = write!(out, " {}: option<{ty}>,", field.name);
            } else {
                let _ = write!(out, " {}: {ty},", field.name);
            }
        }
        out.push_str(" }");
    };
    // `@halo` is between servers only, so clients never hash it.
    let visibility = |out: &mut String, visibility: Visibility, halo: bool| {
        let _ = match visibility {
            Visibility::Public => write!(out, "@visibility(public) "),
            Visibility::Owner => write!(out, "@visibility(owner) "),
            Visibility::None => write!(out, "@visibility(none) "),
            Visibility::Audience(id) => write!(out, "@visibility({}) ", schema.qualified(id)),
        };
        if scope == Scope::Internal {
            let _ = write!(out, "@halo({halo}) ");
        }
    };
    let mut out = String::new();
    for (name, id) in defs {
        match &schema.def(id).kind {
            DefKind::Enum(item) => {
                let _ = write!(out, "enum {name}: {} {{", item.repr);
                for variant in &item.variants {
                    let _ = write!(out, " {} = {},", variant.name, variant.value);
                }
                out.push_str(" }");
            }
            DefKind::Struct(item) => {
                let _ = write!(out, "struct {name}");
                fields(&mut out, &item.fields);
            }
            DefKind::Message(item) => {
                let links: Vec<String> = item
                    .links
                    .iter()
                    .filter(|link| scope == Scope::Internal || link.has(Role::Client))
                    .map(|link| schema.link_name(link))
                    .collect();
                let _ = write!(out, "message({}) {name}", links.join(", "));
                fields(&mut out, &item.fields);
            }
            DefKind::Hub(item) => {
                let _ = write!(out, "hub({}) {name}", schema.type_name(&item.key));
                if scope == Scope::Internal {
                    fields(&mut out, &item.fields);
                }
            }
            DefKind::Union(item) => {
                let _ = write!(out, "union {name} {{");
                for variant in &item.variants {
                    let _ = write!(out, " {}", variant.name);
                    match &variant.data {
                        VariantData::Unit => {}
                        VariantData::Type(ty) => {
                            let _ = write!(out, "({})", schema.type_name(ty));
                        }
                        VariantData::Fields(variant) => fields(&mut out, variant),
                    }
                    out.push(',');
                }
                out.push_str(" }");
            }
            DefKind::Type(item) => {
                let _ = write!(out, "type {name} = {}", schema.type_name(&item.target));
            }
            DefKind::Audience(item) => {
                let _ = write!(out, "audience {name}: {}", schema.type_name(&item.key));
            }
            DefKind::Component(item) => {
                let replicate = match item.replicate {
                    Replicate::Replace => "replace".to_string(),
                    Replicate::Patch => "patch".to_string(),
                    Replicate::Ops(op) => format!("ops({})", schema.qualified(op)),
                };
                let _ = write!(out, "@replicate({replicate}) ");
                visibility(&mut out, item.visibility, item.halo);
                let _ = write!(out, "component {name}");
                fields(&mut out, &item.fields);
            }
            DefKind::Tag(item) => {
                visibility(&mut out, item.visibility, item.halo);
                let _ = write!(out, "tag {name}");
            }
        }
        out.push('\n');
    }

    let mut consts: Vec<(String, ConstId)> = (0..schema.consts.len())
        .map(|index| (schema.const_qualified(ConstId(index)), ConstId(index)))
        .collect();
    consts.sort_by(|left, right| left.0.cmp(&right.0));
    for (name, id) in consts {
        let item = schema.constant(id);
        let _ = write!(out, "const {name}: {} = ", schema.kind_name(&item.ty));
        match &item.value {
            // Escaped by hand, so the hash never follows `{:?}`'s choices.
            ConstValue::String(value) => {
                out.push('"');
                for ch in value.chars() {
                    if matches!(ch, '"' | '\\') {
                        out.push('\\');
                    }
                    out.push(ch);
                }
                out.push('"');
            }
            value => {
                let _ = write!(out, "{value}");
            }
        }
        out.push('\n');
    }
    out
}

/// Which declarations the client fingerprint lists: every message with a
/// client end, each hub kind they name, with what its key reaches, every
/// component and tag clients see, every audience, and whatever they all
/// reach. Hub kinds are listed by key and name.
fn client_listed(schema: &Schema) -> Vec<bool> {
    let mut listed = vec![false; schema.defs.len()];
    let mut todo = Vec::new();
    for (index, def) in schema.defs.iter().enumerate() {
        let visibility = match &def.kind {
            DefKind::Audience(_) => Some(Visibility::Public),
            DefKind::Component(item) => Some(item.visibility),
            DefKind::Tag(item) => Some(item.visibility),
            _ => None,
        };
        if let Some(visibility) = visibility {
            if visibility.reaches_clients() {
                todo.push(TypeId(index));
            }
            continue;
        }
        let DefKind::Message(item) = &def.kind else {
            continue;
        };
        let links: Vec<&Link> = item
            .links
            .iter()
            .filter(|link| link.has(Role::Client))
            .collect();
        if links.is_empty() {
            continue;
        }
        todo.push(TypeId(index));
        for link in links {
            todo.extend(link.from_kinds.iter().flatten().copied());
            todo.extend(link.to_kind);
        }
    }
    while let Some(id) = todo.pop() {
        if listed[id.0] {
            continue;
        }
        listed[id.0] = true;
        let mut types: Vec<&Type> = Vec::new();
        match &schema.def(id).kind {
            DefKind::Hub(item) => types.push(&item.key),
            DefKind::Enum(_) => {}
            DefKind::Struct(item) => types.extend(item.fields.iter().map(|field| &field.ty)),
            DefKind::Message(item) => types.extend(item.fields.iter().map(|field| &field.ty)),
            DefKind::Type(item) => types.push(&item.target),
            DefKind::Union(item) => {
                for variant in &item.variants {
                    match &variant.data {
                        VariantData::Unit => {}
                        VariantData::Type(ty) => types.push(ty),
                        VariantData::Fields(fields) => {
                            types.extend(fields.iter().map(|field| &field.ty));
                        }
                    }
                }
            }
            DefKind::Audience(item) => types.push(&item.key),
            DefKind::Component(item) => {
                types.extend(item.fields.iter().map(|field| &field.ty));
                if let Replicate::Ops(op) = item.replicate {
                    todo.push(op);
                }
                if let Visibility::Audience(audience) = item.visibility {
                    todo.push(audience);
                }
            }
            DefKind::Tag(item) => {
                if let Visibility::Audience(audience) = item.visibility {
                    todo.push(audience);
                }
            }
        }
        for ty in types {
            named(ty, &mut todo);
        }
    }
    listed
}

/// The declarations `ty` names, through collections.
pub(super) fn named(ty: &Type, out: &mut Vec<TypeId>) {
    match &ty.kind {
        TypeKind::Array(item, _) | TypeKind::Vec(item) | TypeKind::Set(item) => named(item, out),
        TypeKind::Map(key, value) => {
            named(key, out);
            named(value, out);
        }
        TypeKind::Named(id) => out.push(*id),
        _ => {}
    }
}

/// 64-bit FNV-1a: tiny, stable, and enough to tell schemas apart. It is not
/// collision-resistant against a deliberate attacker, and need not be.
pub(super) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::wire::tests::*;

    #[test]
    fn canonical_text_spells_out_unions_and_types() {
        let (schema, _) = compiled(&[concat!(
            "namespace a\n",
            "type PartyId = u64\n",
            "union Op { Add { id: PartyId, note: option<string> }, Remove(u8[max=9]), Clear }\n",
        )]);
        assert_eq!(
            canonical(&schema, Scope::Internal),
            concat!(
                "union a.Op { Add { id: a.PartyId, note: option<string>, }, Remove(u8[max=9]), Clear, }\n",
                "type a.PartyId = u64\n",
            )
        );
    }

    #[test]
    fn canonical_text_spells_times_by_value() {
        let (schema, _) = compiled(&[concat!(
            "namespace a\n",
            "const D: duration = 1000ms\n",
            "const L: duration = 90m\n",
            "const O: duration = 1001ms\n",
            "const T: timestamp = 2026-11-07T12:00:00.000000Z\n",
            "const U: timestamp = 2026-11-07T12:00:00.250000Z\n",
            "const Z: duration = 0h\n",
        )]);
        assert_eq!(
            canonical(&schema, Scope::Internal),
            concat!(
                "const a.D: duration = 1s\n",
                "const a.L: duration = 90m\n",
                "const a.O: duration = 1001ms\n",
                "const a.T: timestamp = 2026-11-07T12:00:00Z\n",
                "const a.U: timestamp = 2026-11-07T12:00:00.250Z\n",
                "const a.Z: duration = 0s\n",
            )
        );
    }

    #[test]
    fn fingerprints_cover_unions_and_types() {
        let fingerprint = |src: &str| {
            compiled(&[&format!("namespace a\n{src}\n")])
                .1
                .fingerprints()
                .internal
        };
        let base = fingerprint(concat!(
            "type Id = u32\n",
            "union U { A { x: Id }, B(u8), C }\n",
        ));
        assert_eq!(
            base,
            fingerprint(concat!(
                "union U { A { x: Id, }, B(u8), C, }\n",
                "type Id = u32\n",
            )),
            "declaration order and trailing commas do not matter"
        );
        for changed in [
            // Variants: order, names, and forms.
            "type Id = u32\nunion U { B(u8), A { x: Id }, C }",
            "type Id = u32\nunion U { A { x: Id }, B(u8), D }",
            "type Id = u32\nunion U { A { y: Id }, B(u8), C }",
            "type Id = u32\nunion U { A(Id), B(u8), C }",
            "type Id = u32\nunion U { A { x: Id }, B(u16), C }",
            "type Id = u32\nunion U { A { x: Id }, B(u8[max=9]), C }",
            "type Id = u32\nunion U { A { x: Id }, B(u8), C, D }",
            "type Id = u32\nunion V { A { x: Id }, B(u8), C }",
            // A type's target and name.
            "type Id = u64\nunion U { A { x: Id }, B(u8), C }",
            "type Id = u32[max=9]\nunion U { A { x: Id }, B(u8), C }",
            "type Key = u32\nunion U { A { x: Key }, B(u8), C }",
            // A type, or the scalar it stands for.
            "type Id = u32\nunion U { A { x: u32 }, B(u8), C }",
        ] {
            assert_ne!(base, fingerprint(changed), "{changed}");
        }
    }

    #[test]
    fn canonical_text_normalizes_links() {
        let canonical_of = |src: &str| {
            let (schema, _) = compiled(&[&format!(
                "namespace a\nhub(u64) Party {{}}\nhub(u32) Guild {{}}\n{src}\n"
            )]);
            let text = canonical(&schema, Scope::Internal);
            let line = text.lines().find(|line| line.starts_with("message"));
            line.unwrap().to_string()
        };
        assert_eq!(
            canonical_of("message(hub<Party> | hub<Guild> -> client) M {}"),
            "message(hub<a.Guild> | hub<a.Party> -> client) a.M { }",
            "sending kinds are sorted by qualified name"
        );
        assert_eq!(
            canonical_of("message(client <-> hub<Party>) M {}"),
            "message(client -> hub<a.Party>, hub<a.Party> -> client) a.M { }"
        );
        assert_eq!(
            canonical_of("message(hub<Party> <-> client) M {}"),
            canonical_of("message(client <-> hub<Party>) M {}")
        );
        assert_eq!(
            canonical_of("message(hub | sim -> entry | client) M {}"),
            "message(sim -> client, sim -> entry, hub -> client, hub -> entry) a.M { }"
        );
        assert_eq!(
            canonical_of("message(sim <-> sim) M {}"),
            "message(sim -> sim) a.M { }"
        );
    }

    #[test]
    fn the_client_fingerprint_lists_what_clients_see() {
        let (schema, _) = compiled(&[concat!(
            "namespace a\n",
            "type PartyId = u64\n",
            "type Exp = u32\n",
            "struct Seen { e: E }\n",
            "struct Unseen { x: u8 }\n",
            "enum E { A }\n",
            "hub(PartyId) Party { members: vec<u64> }\n",
            "hub(u32) Guild { name: string }\n",
            "hub(u16) Bank { gold: u64 }\n",
            "message(client -> hub<Party> | sim) Join { seen: Seen }\n",
            "message(hub<Guild> -> client | sim) Notice {}\n",
            "message(sim -> hub<Bank>) Deposit { exp: Exp }\n",
            "const C: u8 = 1\n",
        )]);
        // Kinds by header only, and only the links with a client end.
        assert_eq!(
            canonical(&schema, Scope::Client),
            concat!(
                "enum a.E: u32 { A = 0, }\n",
                "hub(u32) a.Guild\n",
                "message(client -> sim, client -> hub<a.Party>) a.Join { seen: a.Seen, }\n",
                "message(hub<a.Guild> -> client) a.Notice { }\n",
                "hub(a.PartyId) a.Party\n",
                "type a.PartyId = u64\n",
                "struct a.Seen { e: a.E, }\n",
                "const a.C: u8 = 1\n",
            )
        );
    }

    #[test]
    fn server_links_leave_the_client_fingerprint_alone() {
        let fingerprints = |src: &str| {
            compiled(&[&format!(
                "namespace a\nhub(u64) P {{ x: u8 }}\nmessage(client -> sim) M {{}}\n{src}\n"
            )])
            .1
            .fingerprints()
        };
        let base = fingerprints("message(sim -> hub<P>) N { x: u8 }");
        for changed in [
            "message(sim -> hub<P>) N { x: u16 }",
            "message(sim -> sim) N { x: u8 }",
            "message(hub -> sim) N { x: u8 }",
            "hub(u8) Q {}\nmessage(sim -> hub<P>) N { x: u8 }",
        ] {
            let fingerprints = fingerprints(changed);
            assert_eq!(base.client, fingerprints.client, "{changed}");
            assert_ne!(base.internal, fingerprints.internal, "{changed}");
        }
        for changed in [
            "message(client -> sim) N { x: u8 }",
            "message(sim -> client) N { x: u8 }",
            "message(sim | client -> hub<P>) N { x: u8 }",
            "const C: u8 = 1\nmessage(sim -> hub<P>) N { x: u8 }",
        ] {
            assert_ne!(base.client, fingerprints(changed).client, "{changed}");
        }
        // A kind's body is never in the client fingerprint, its key is.
        let with_kind = |kind: &str| {
            compiled(&[&format!(
                "namespace a\n{kind}\nmessage(client -> hub<P>) M {{}}\n"
            )])
            .1
            .fingerprints()
        };
        let base = with_kind("hub(u64) P { x: u8 }");
        assert_eq!(base.client, with_kind("hub(u64) P { y: u16 }").client);
        assert_ne!(base.internal, with_kind("hub(u64) P { y: u16 }").internal);
        assert_ne!(base.client, with_kind("hub(u32) P { x: u8 }").client);
    }

    #[test]
    fn canonical_text_spells_out_hub_kinds() {
        let (schema, _) = compiled(&[concat!(
            "namespace a\n",
            "type GuildId = u64\n",
            "hub(GuildId) Guild { enum Rank { A } name: string[max_length=24], rank: option<Rank> }\n",
            "hub(u32[max=9]) Plot {}\n",
        )]);
        assert_eq!(
            canonical(&schema, Scope::Internal),
            concat!(
                "hub(a.GuildId) a.Guild { name: string[max_length=24], rank: option<a.Guild.Rank>, }\n",
                "enum a.Guild.Rank: u32 { A = 0, }\n",
                "type a.GuildId = u64\n",
                "hub(u32[max=9]) a.Plot { }\n",
            )
        );
    }

    #[test]
    fn fingerprints_cover_hub_kinds() {
        let fingerprint = |src: &str| {
            compiled(&[&format!("namespace a\n{src}\n")])
                .1
                .fingerprints()
                .internal
        };
        let base = fingerprint("type Id = u64\nhub(Id) G { x: u8 }");
        assert_eq!(base, fingerprint("hub(Id) G { x: u8, }\ntype Id = u64"));
        for changed in [
            // The key, by name and by type.
            "type Id = u64\nhub(u64) G { x: u8 }",
            "type Id = u32\nhub(Id) G { x: u8 }",
            "type Key = u64\nhub(Key) G { x: u8 }",
            // The kind's name and fields.
            "type Id = u64\nhub(Id) H { x: u8 }",
            "type Id = u64\nhub(Id) G { y: u8 }",
            // A struct is not a kind.
            "type Id = u64\nstruct G { x: u8 }",
        ] {
            assert_ne!(base, fingerprint(changed), "{changed}");
        }
    }

    const COMPONENTS: &str = concat!(
        "namespace a\n",
        "type FactionId = u32\n",
        "audience Faction: FactionId\n",
        "audience Stealth: u8\n",
        "struct Item { id: u32 }\n",
        "struct Secret { x: u8 }\n",
        "union Op { Add(Item), Clear }\n",
        "component Health { hp: u32 }\n",
        "@replicate(ops(Op)) @visibility(owner) @halo(false)\n",
        "component Inventory { items: vec<Item> }\n",
        "@replicate(patch) @visibility(none)\n",
        "component Brain { secret: Secret }\n",
        "@visibility(Faction) component Chatter { x: u8 }\n",
        "@visibility(Stealth) tag Hidden\n",
        "@visibility(none) tag Busy\n",
    );

    #[test]
    fn canonical_text_spells_out_components_tags_and_audiences() {
        let (schema, _) = compiled(&[COMPONENTS]);
        assert_eq!(
            canonical(&schema, Scope::Internal),
            concat!(
                "@replicate(patch) @visibility(none) @halo(false) component a.Brain { secret: a.Secret, }\n",
                "@visibility(none) @halo(false) tag a.Busy\n",
                "@replicate(replace) @visibility(a.Faction) @halo(true) component a.Chatter { x: u8, }\n",
                "audience a.Faction: a.FactionId\n",
                "type a.FactionId = u32\n",
                "@replicate(replace) @visibility(public) @halo(true) component a.Health { hp: u32, }\n",
                "@visibility(a.Stealth) @halo(true) tag a.Hidden\n",
                "@replicate(ops(a.Op)) @visibility(owner) @halo(false) component a.Inventory { items: vec<a.Item>, }\n",
                "struct a.Item { id: u32, }\n",
                "union a.Op { Add(a.Item), Clear, }\n",
                "struct a.Secret { x: u8, }\n",
                "audience a.Stealth: u8\n",
            )
        );
        // Clients see no `@halo`, nothing with `visibility=none`, and nothing
        // only those reach.
        assert_eq!(
            canonical(&schema, Scope::Client),
            concat!(
                "@replicate(replace) @visibility(a.Faction) component a.Chatter { x: u8, }\n",
                "audience a.Faction: a.FactionId\n",
                "type a.FactionId = u32\n",
                "@replicate(replace) @visibility(public) component a.Health { hp: u32, }\n",
                "@visibility(a.Stealth) tag a.Hidden\n",
                "@replicate(ops(a.Op)) @visibility(owner) component a.Inventory { items: vec<a.Item>, }\n",
                "struct a.Item { id: u32, }\n",
                "union a.Op { Add(a.Item), Clear, }\n",
                "audience a.Stealth: u8\n",
            )
        );
    }

    #[test]
    fn every_audience_is_in_the_client_fingerprint() {
        let (schema, _) = compiled(&["namespace a\nstruct K {}\naudience Unused: u8\n"]);
        assert_eq!(canonical(&schema, Scope::Client), "audience a.Unused: u8\n");
    }

    #[test]
    fn fingerprints_cover_components_tags_and_audiences() {
        let fingerprints = |src: &str| {
            compiled(&[&format!("{COMPONENTS}{src}\n")])
                .1
                .fingerprints()
        };
        let base = fingerprints("");
        // Server-only changes leave clients alone.
        for changed in [
            "@visibility(none) component Extra { x: u8 }",
            "@visibility(none) tag Extra",
        ] {
            let changed_fingerprints = fingerprints(changed);
            assert_eq!(base.client, changed_fingerprints.client, "{changed}");
            assert_ne!(base.internal, changed_fingerprints.internal, "{changed}");
        }
        let swap = |from: &str, to: &str| {
            assert!(COMPONENTS.contains(from), "{from}");
            compiled(&[&COMPONENTS.replace(from, to)]).1.fingerprints()
        };
        for (from, to) in [
            // Halo, and what only a `visibility=none` component reaches.
            ("@halo(false)\n", "\n"),
            ("component Health", "@halo(false) component Health"),
            ("struct Secret { x: u8 }", "struct Secret { x: u16 }"),
            (
                "component Brain { secret: Secret }",
                "component Brain { secret: Secret, y: u8 }",
            ),
            ("@visibility(none) tag Busy", "@visibility(none) tag Idle"),
            ("@replicate(patch) @visibility(none)", "@visibility(none)"),
        ] {
            let changed = swap(from, to);
            assert_eq!(base.client, changed.client, "{from} -> {to}");
            assert_ne!(base.internal, changed.internal, "{from} -> {to}");
        }
        // What clients see changes both.
        for (from, to) in [
            (
                "component Health { hp: u32 }",
                "component Health { hp: u16 }",
            ),
            ("component Health", "@replicate(patch) component Health"),
            ("component Health", "@visibility(owner) component Health"),
            ("@visibility(Stealth) tag Hidden", "tag Hidden"),
            ("@visibility(Faction)", "@visibility(Stealth)"),
            ("audience Stealth: u8", "audience Stealth: u16"),
            ("type FactionId = u32", "type FactionId = u64"),
            ("union Op { Add(Item), Clear }", "union Op { Add(Item) }"),
            ("struct Item { id: u32 }", "struct Item { id: u64 }"),
            ("@visibility(none) tag Busy", "tag Busy"),
            ("@visibility(none) tag Busy", "@visibility(public) tag Busy"),
        ] {
            let changed = swap(from, to);
            assert_ne!(base.client, changed.client, "{from} -> {to}");
            assert_ne!(base.internal, changed.internal, "{from} -> {to}");
        }
        // Defaults hash as if spelled out.
        assert_eq!(
            base,
            swap(
                "component Health",
                "@replicate(replace) @visibility(public) @halo(true) component Health"
            )
        );
        assert_eq!(
            base,
            swap(
                "@visibility(none) tag Busy",
                "@visibility(none) @halo(false) tag Busy"
            )
        );
    }

    #[test]
    fn fingerprints_ignore_source_order() {
        let (_, forward) = compiled(&[EXAMPLE1, EXAMPLE2]);
        let (_, backward) = compiled(&[EXAMPLE2, EXAMPLE1]);
        assert_eq!(forward.fingerprints(), backward.fingerprints());
    }

    #[test]
    fn fingerprints_cover_names_layout_and_constraints() {
        let fingerprint = |src: &str| {
            compiled(&[&format!("namespace a\n{src}\n")])
                .1
                .fingerprints()
                .internal
        };
        let base = fingerprint("message(client -> sim) M { x: u8, y: u16 }");
        assert_eq!(
            base,
            fingerprint("message(client -> sim) M { x: u8, y: u16 }")
        );
        for changed in [
            "message(client -> sim) M { x: u8, z: u16 }",
            "message(client -> sim) N { x: u8, y: u16 }",
            "message(client -> sim) M { y: u16, x: u8 }",
            "message(sim -> client) M { x: u8, y: u16 }",
            "message(client -> sim) M { x: u8[max=9], y: u16 }",
            "message(client -> sim) M { x: option<u8>, y: u16 }",
            "message(client -> sim) M { x: u8, y: u16 }\nenum E { A }",
            "message(client -> sim) M { x: u8, y: u16 }\nconst C: u8 = 1",
            "message(client -> sim) M { const C: u8 = 1 x: u8, y: u16 }",
        ] {
            assert_ne!(base, fingerprint(changed), "{changed}");
        }
    }

    #[test]
    fn fingerprints_cover_const_values() {
        let fingerprint = |src: &str| {
            compiled(&[&format!("namespace a\n{src}\n")])
                .1
                .fingerprints()
                .internal
        };
        let base = fingerprint("const C: u8 = 1\nconst S: string = \"a\"");
        for changed in [
            "const C: u8 = 2\nconst S: string = \"a\"",
            "const C: u16 = 1\nconst S: string = \"a\"",
            "const D: u8 = 1\nconst S: string = \"a\"",
            "const C: u8 = 1\nconst S: string = \"b\"",
        ] {
            assert_ne!(base, fingerprint(changed), "{changed}");
        }
        // Unescaped, the first would read exactly as the second.
        assert_ne!(
            fingerprint(r#"const A: string = "x\"\nconst a.B: string = \"y""#),
            fingerprint("const A: string = \"x\"\nconst B: string = \"y\""),
        );

        // Durations and timestamps count by value, not by spelling.
        assert_eq!(
            fingerprint("const D: duration = 1000ms"),
            fingerprint("const D: duration = 1s")
        );
        assert_ne!(
            fingerprint("const D: duration = 1s"),
            fingerprint("const D: duration = 2s")
        );
        assert_eq!(
            fingerprint("const T: timestamp = 2026-01-01T00:00:00.000Z"),
            fingerprint("const T: timestamp = 2026-01-01T00:00:00Z")
        );
        assert_ne!(
            fingerprint("const T: timestamp = 2026-01-01T00:00:00Z"),
            fingerprint("const T: timestamp = 2026-01-01T00:00:01Z")
        );
    }
}
