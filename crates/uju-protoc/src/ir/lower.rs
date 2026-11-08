mod attrs;
mod checks;
mod consts;
mod links;
mod types;

use std::collections::{BTreeMap, HashSet};

use crate::diagnostic::{Diagnostic, SourceId};
use crate::ir::wire::{self, Wire};
use crate::ir::*;
use crate::lexer::Span;
use crate::parser::ast;
use crate::resolve::{ConstSite, Resolver, TypeSite};

use checks::FixedCheck;

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

    let mut schema = Schema {
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

    let qualified: Vec<String> = (0..schema.defs.len())
        .map(|index| schema.qualified(TypeId(index)))
        .collect();
    for def in &mut schema.defs {
        if let DefKind::Message(item) = &mut def.kind {
            for kinds in item
                .links
                .iter_mut()
                .filter_map(|link| link.from_kinds.as_mut())
            {
                kinds.sort_by(|left, right| qualified[left.0].cmp(&qualified[right.0]));
            }
        }
    }

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

impl<'a, 'src> Lowerer<'a, 'src> {
    fn error(&mut self, source: SourceId, span: &Span, message: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::new(source, span.clone(), message));
    }

    fn lower_def(&mut self, id: TypeId, site: &TypeSite<'a, 'src>) -> Def {
        self.check_attrs(site.source, site.item.attrs(), site.item.keyword());
        let kind = match site.item {
            ast::Item::Enum(item) => DefKind::Enum(self.lower_enum(site.source, item)),
            ast::Item::Struct(item) => DefKind::Struct(self.lower_struct(site.source, id, item)),
            ast::Item::Message(item) => DefKind::Message(self.lower_message(site.source, id, item)),
            ast::Item::Union(item) => DefKind::Union(self.lower_union(site.source, id, item)),
            ast::Item::Type(item) => DefKind::Type(self.lower_type_def(site.source, id, item)),
            ast::Item::Hub(item) => DefKind::Hub(self.lower_hub(site.source, id, item)),
            ast::Item::Const(_) => unreachable!("consts are not type sites"),
            ast::Item::Audience(item) => {
                DefKind::Audience(self.lower_audience(site.source, id, item))
            }
            ast::Item::Component(item) => {
                DefKind::Component(self.lower_component(site.source, id, item))
            }
            ast::Item::Tag(item) => DefKind::Tag(self.lower_tag(site.source, id, item)),
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

    /// A hub kind's key is resolved where the kind is declared, at the top
    /// of its namespace, and must be fit to look objects up by.
    fn lower_hub(&mut self, source: SourceId, id: TypeId, item: &ast::Hub<'src>) -> Hub {
        let written = match &item.key.kind {
            ast::TypeKind::Option(inner) => {
                self.error(source, &item.key.span, "a hub key cannot be optional");
                &**inner
            }
            _ => &item.key,
        };
        let scope = self.resolver.types[id.0].parent;
        let key = self.lower_type(source, scope, written);
        self.check_key(source, scope, written, &key, "a hub key");
        Hub {
            key,
            fields: self.lower_fields(source, id, &item.body.fields),
        }
    }

    /// An audience's key is resolved and checked as a hub kind's is.
    fn lower_audience(
        &mut self,
        source: SourceId,
        id: TypeId,
        item: &ast::Audience<'src>,
    ) -> Audience {
        let written = match &item.key.kind {
            ast::TypeKind::Option(inner) => {
                self.error(source, &item.key.span, "an audience key cannot be optional");
                &**inner
            }
            _ => &item.key,
        };
        let scope = self.resolver.types[id.0].parent;
        let key = self.lower_type(source, scope, written);
        self.check_key(source, scope, written, &key, "an audience key");
        Audience { key }
    }

    /// Attribute paths resolve where the component is declared, as a hub
    /// key does, and its fields inside it.
    fn lower_component(
        &mut self,
        source: SourceId,
        id: TypeId,
        item: &ast::Component<'src>,
    ) -> Component {
        let name = item.name.0;
        if item.body.fields.is_empty() {
            self.error(
                source,
                &item.name.1,
                format!("a component with no fields is a tag; write `tag {name}`"),
            );
        }
        let scope = self.resolver.types[id.0].parent;
        let replicate = self.lower_replicate(source, scope, &item.attrs);
        let visibility = self.lower_visibility(source, scope, &item.attrs);
        let halo = self.lower_halo(source, &item.attrs, visibility);
        Component {
            fields: self.lower_fields(source, id, &item.body.fields),
            replicate,
            visibility,
            halo,
        }
    }

    fn lower_tag(&mut self, source: SourceId, id: TypeId, item: &ast::Tag<'src>) -> Tag {
        let scope = self.resolver.types[id.0].parent;
        let visibility = self.lower_visibility(source, scope, &item.attrs);
        let halo = self.lower_halo(source, &item.attrs, visibility);
        Tag { visibility, halo }
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
        Message {
            links: self.lower_links(source, id, &item.endpoints),
            fields: self.lower_fields(source, id, &item.body.fields),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::compile;

    pub(super) const EXAMPLE1: &str = include_str!("../../tests/data/example1.uju");

    pub(super) const EXAMPLE2: &str = include_str!("../../tests/data/example2.uju");

    pub(super) fn compiled(sources: &[&str]) -> Schema {
        compile(sources)
            .unwrap_or_else(|errors| panic!("unexpected errors: {errors:#?}"))
            .schema
    }

    pub(super) fn messages(sources: &[&str]) -> Vec<String> {
        compile(sources)
            .expect_err("expected errors")
            .into_iter()
            .filter(|diagnostic| diagnostic.is_error())
            .map(|diagnostic| diagnostic.message)
            .collect()
    }

    pub(super) fn def<'a>(schema: &'a Schema, name: &str) -> (TypeId, &'a Def) {
        let index = schema
            .defs
            .iter()
            .position(|def| def.name == name)
            .unwrap_or_else(|| panic!("no declaration named `{name}`"));
        (TypeId(index), &schema.defs[index])
    }

    pub(super) fn constant<'a>(schema: &'a Schema, name: &str) -> &'a Const {
        schema
            .consts
            .iter()
            .find(|item| item.name == name)
            .unwrap_or_else(|| panic!("no const named `{name}`"))
    }

    /// Each error's message and the source it points at.
    pub(super) fn spanned(src: &str) -> Vec<(String, String)> {
        compile(&[src])
            .expect_err("expected errors")
            .into_iter()
            .filter(|diagnostic| diagnostic.is_error())
            .map(|diagnostic| (src[diagnostic.span.clone()].to_string(), diagnostic.message))
            .collect()
    }

    pub(super) fn component<'a>(schema: &'a Schema, name: &str) -> &'a Component {
        let DefKind::Component(item) = &def(schema, name).1.kind else {
            panic!("`{name}` is not a component");
        };
        item
    }

    pub(super) fn tag<'a>(schema: &'a Schema, name: &str) -> &'a Tag {
        let DefKind::Tag(item) = &def(schema, name).1.kind else {
            panic!("`{name}` is not a tag");
        };
        item
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
        let src = "namespace a\nenum R { X }\nmessage(client -> sim) M {\n    enum R { Y }\n    r: R,\n}\n";
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
        compiled(&[
            "namespace a\nmessage(client -> sim) M { enum R { Y } r: R, }\nstruct S { r: M.R }\n",
        ]);
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

    #[test]
    fn messages_are_standalone() {
        assert_eq!(
            messages(&["namespace a\nmessage(client -> sim) M {}\nstruct S { m: M }\n"]),
            ["`M` is a message; messages are standalone and cannot be used as a type"]
        );
        assert_eq!(
            messages(&[
                "namespace a\nmessage(client -> sim) M {}\nmessage(sim -> client) N { v: vec<M> }\n"
            ])
            .len(),
            1
        );
    }

    #[test]
    fn structs_can_hold_variable_size_and_optional_fields() {
        compiled(&["namespace a\nstruct S { v: vec<i32>, s: string, x: option<i32> }\n"]);
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
            "message(client -> sim) M { type Local = bool p: PartyId, l: Local }\n",
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
                "message(client -> sim) M {}\nunion U { A(M) }",
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
            let src = format!("{prelude}message(client -> sim) M {{ {field} }}\n");
            assert_eq!(messages(&[&src]), [expected], "{field}");
        }

        // A union is fine wherever a variable-size value is, and a `type`
        // wherever its target is.
        compiled(&[&format!(
            "{prelude}message(client -> sim) M {{ v: vec<U>, m: map<u8, U>, o: option<U>, \
             a: array<PartyId, 2>, s: set<PartyId>, k: map<PartyId, Weight>, \
             w: vec<Weight>, p: option<PartyId> }}\n"
        )]);
    }

    #[test]
    fn hub_kinds_lower_with_their_key_and_fields() {
        let schema = compiled(&[concat!(
            "namespace a\n",
            "type GuildId = u64\n",
            "hub(GuildId) Guild {\n",
            "    enum Rank { Member, Officer }\n",
            "    name: string[max_length=24],\n",
            "    rank: Rank,\n",
            "    motto: option<string>,\n",
            "}\n",
            "hub(u32[max=9]) Plot { x: u8 }\n",
        )]);
        let (guild, def_) = def(&schema, "Guild");
        let DefKind::Hub(hub) = &def_.kind else {
            panic!("expected a hub kind");
        };
        assert_eq!(def_.parent, None);
        assert_eq!(schema.type_name(&hub.key), "a.GuildId");
        assert_eq!(
            hub.fields
                .iter()
                .map(|field| (
                    field.name.as_str(),
                    schema.type_name(&field.ty),
                    field.optional
                ))
                .collect::<Vec<_>>(),
            [
                ("name", "string[max_length=24]".to_string(), false),
                ("rank", "a.Guild.Rank".to_string(), false),
                ("motto", "string".to_string(), true),
            ]
        );
        assert_eq!(def(&schema, "Rank").1.parent, Some(guild));

        let (plot, def_) = def(&schema, "Plot");
        let DefKind::Hub(hub) = &def_.kind else {
            panic!("expected a hub kind");
        };
        assert_eq!(schema.type_name(&hub.key), "u32[max=9]");
        assert!(schema.is_fixed(&TypeKind::Named(plot).into()));
        assert!(!schema.is_fixed(&TypeKind::Named(guild).into()));
    }

    #[test]
    fn hub_kinds_are_not_types() {
        let error = "a hub kind is not a type; declare a struct for what you send";
        for (decl, written) in [
            ("struct S { h: H }", "H"),
            ("message(client -> sim) M { h: option<H> }", "H"),
            ("struct S { v: vec<a.H> }", "a.H"),
            ("struct S { a: array<H, 2> }", "H"),
            ("struct S { s: set<H> }", "H"),
            ("struct S { m: map<H, u8> }", "H"),
            ("struct S { m: map<u8, H> }", "H"),
            ("union U { A(H) }", "H"),
            ("union U { A { h: H } }", "H"),
            ("type T = H", "H"),
            ("hub(H) K {}", "H"),
            ("hub(u8) K { k: K }", "K"),
        ] {
            let src = format!("namespace a\nhub(u8) H {{ x: u8 }}\n{decl}\n");
            assert_eq!(
                spanned(&src),
                [(written.to_string(), error.to_string())],
                "{decl}"
            );
        }
        // What a kind shows others is a struct of its own.
        compiled(&[concat!(
            "namespace a\n",
            "hub(u8) H { x: u8, secret: u64 }\n",
            "struct Info { x: u8 }\n",
            "message(hub<H> -> client) M { info: Info }\n",
        )]);
    }

    #[test]
    fn components_tags_and_audiences_lower_with_their_attributes() {
        let schema = compiled(&[concat!(
            "namespace a\n",
            "type FactionId = u32\n",
            "audience Faction: FactionId\n",
            "audience Stealth: u8\n",
            "union Op { Add(u8), Clear }\n",
            "component Plain { x: u8 }\n",
            "@replicate(replace) @visibility(public) @halo(true)\n",
            "component Spelled { x: u8 }\n",
            "@replicate(patch) @visibility(owner)\n",
            "component Stats { hp: u32, mp: u32 }\n",
            "@replicate(ops(Op)) @visibility(none)\n",
            "component Inventory { items: vec<u8> }\n",
            "@visibility(none) @halo(true)\n",
            "component Shared { x: u8 }\n",
            "@visibility(Faction) @halo(false)\n",
            "component Chatter { x: u8 }\n",
            "tag Dead\n",
            "@visibility(a.Stealth)\n",
            "tag Hidden\n",
            "@visibility(none)\n",
            "tag Marked\n",
        )]);
        let (op, _) = def(&schema, "Op");
        let (faction, faction_def) = def(&schema, "Faction");
        let (stealth, _) = def(&schema, "Stealth");
        let DefKind::Audience(audience) = &faction_def.kind else {
            panic!("expected an audience");
        };
        assert_eq!(schema.type_name(&audience.key), "a.FactionId");

        let summary = |item: &Component| (item.replicate, item.visibility, item.halo);
        let plain = component(&schema, "Plain");
        assert_eq!(
            summary(plain),
            (Replicate::Replace, Visibility::Public, true)
        );
        assert_eq!(plain.fields.len(), 1);
        assert_eq!(summary(component(&schema, "Spelled")), summary(plain));
        assert_eq!(
            summary(component(&schema, "Stats")),
            (Replicate::Patch, Visibility::Owner, true)
        );
        // No client sees it, so neighbor shards don't get it either unless
        // asked.
        assert_eq!(
            summary(component(&schema, "Inventory")),
            (Replicate::Ops(op), Visibility::None, false)
        );
        assert_eq!(
            summary(component(&schema, "Shared")),
            (Replicate::Replace, Visibility::None, true)
        );
        assert_eq!(
            summary(component(&schema, "Chatter")),
            (Replicate::Replace, Visibility::Audience(faction), false)
        );

        let summary = |item: &Tag| (item.visibility, item.halo);
        assert_eq!(summary(tag(&schema, "Dead")), (Visibility::Public, true));
        assert_eq!(
            summary(tag(&schema, "Hidden")),
            (Visibility::Audience(stealth), true)
        );
        assert_eq!(summary(tag(&schema, "Marked")), (Visibility::None, false));
    }

    #[test]
    fn a_component_without_fields_is_a_tag() {
        assert_eq!(
            spanned("namespace a\ncomponent Dead {}\n"),
            [(
                "Dead".to_string(),
                "a component with no fields is a tag; write `tag Dead`".to_string()
            )]
        );
        assert_eq!(
            messages(&["namespace a\ncomponent Dead { const X: u8 = 1 }\n"]),
            ["a component with no fields is a tag; write `tag Dead`"]
        );
    }

    #[test]
    fn components_tags_and_audiences_are_not_types() {
        let prelude = concat!(
            "namespace a\n",
            "component C { x: u8 }\n",
            "tag T\n",
            "audience A: u8\n",
        );
        let cases = [
            (
                "C",
                "a component is not a type; declare a struct for what you send",
            ),
            ("T", "a tag is not a type; it has no value"),
            (
                "A",
                "an audience is not a type; name its key's type instead",
            ),
        ];
        for (name, error) in cases {
            for decl in [
                format!("struct S {{ x: {name} }}"),
                format!("message(sim -> client) M {{ x: option<{name}> }}"),
                format!("struct S {{ x: vec<a.{name}> }}"),
                format!("struct S {{ x: map<u8, {name}> }}"),
                format!("union U {{ X({name}) }}"),
                format!("type X = {name}"),
                format!("hub({name}) H {{}}"),
                format!("audience K: {name}"),
                format!("component K {{ x: {name} }}"),
            ] {
                let src = format!("{prelude}{decl}\n");
                let written = if decl.contains("a.") {
                    format!("a.{name}")
                } else {
                    name.to_string()
                };
                assert_eq!(spanned(&src), [(written, error.to_string())], "{decl}");
            }
        }
        // A component can't name itself either.
        assert_eq!(
            messages(&["namespace a\ncomponent C { c: vec<C> }\n"]),
            ["a component is not a type; declare a struct for what you send"]
        );
        // Nor are they hub kinds.
        assert_eq!(
            messages(&[&format!("{prelude}message(sim -> hub<C>) M {{}}\n")]),
            ["`C` is not a hub kind"]
        );
    }

    #[test]
    fn component_fields_are_checked_as_a_struct_s() {
        assert_eq!(
            messages(&["namespace a\ncomponent C { x: u8, x: u8 }\n"]),
            ["duplicate field `x`"]
        );
        assert_eq!(
            messages(&["namespace a\ncomponent C { x: array<u64, 9000> }\n"]),
            ["`array<u64, 9000>` takes 72000 bytes; a message holds at most 65535"]
        );
        // Recursion through a nested declaration is still recursion.
        assert_eq!(
            messages(&["namespace a\ncomponent C { s: S, struct S { s: vec<S> } }\n"]),
            ["`S` refers to itself; recursive types are not allowed"]
        );
    }
}
