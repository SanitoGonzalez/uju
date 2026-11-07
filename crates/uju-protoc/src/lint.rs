//! Advisory checks: what compiles but is likely a mistake, reported as
//! warnings.

use crate::case;
use crate::diagnostic::{Diagnostic, SourceId};
use crate::parser::ast;

/// Warns about names that break the schema's naming convention:
/// namespaces and fields in `snake_case`, types, enum variants, and consts
/// in `PascalCase`. Backends convert names into their own conventions, which
/// is only predictable from a consistent starting point.
pub fn naming(source: SourceId, schema: &ast::Schema<'_>, out: &mut Vec<Diagnostic>) {
    for segment in &schema.namespace.segments {
        expect(source, "namespace", segment, Style::Snake, out);
    }
    for item in &schema.items {
        item_naming(source, item, out);
    }
}

fn item_naming(source: SourceId, item: &ast::Item<'_>, out: &mut Vec<Diagnostic>) {
    match item {
        ast::Item::Enum(item) => {
            expect(source, "enum", &item.name, Style::Pascal, out);
            for variant in &item.variants {
                expect(source, "variant", &variant.name, Style::Pascal, out);
            }
        }
        ast::Item::Struct(item) => {
            expect(source, "struct", &item.name, Style::Pascal, out);
            body_naming(source, &item.body, out);
        }
        ast::Item::Message(item) => {
            expect(source, "message", &item.name, Style::Pascal, out);
            body_naming(source, &item.body, out);
        }
        ast::Item::Const(item) => expect(source, "const", &item.name, Style::Pascal, out),
        ast::Item::Union(item) => {
            expect(source, "union", &item.name, Style::Pascal, out);
            for variant in &item.variants {
                expect(source, "variant", &variant.name, Style::Pascal, out);
                if let ast::VariantData::Fields { fields, .. } = &variant.data {
                    for field in fields {
                        expect(source, "field", &field.name, Style::Snake, out);
                    }
                }
            }
        }
        ast::Item::Type(item) => expect(source, "type", &item.name, Style::Pascal, out),
    }
}

fn body_naming(source: SourceId, body: &ast::Body<'_>, out: &mut Vec<Diagnostic>) {
    for field in &body.fields {
        expect(source, "field", &field.name, Style::Snake, out);
    }
    for item in &body.items {
        item_naming(source, item, out);
    }
}

#[derive(Clone, Copy)]
enum Style {
    Snake,
    Pascal,
}

fn expect(
    source: SourceId,
    what: &str,
    (name, span): &ast::Ident<'_>,
    style: Style,
    out: &mut Vec<Diagnostic>,
) {
    let (suggestion, style) = match style {
        Style::Snake => (case::snake(name), "snake_case"),
        Style::Pascal => (pascal_suggestion(name), "PascalCase"),
    };
    if suggestion == *name {
        return;
    }
    let mut message = format!("{what} `{name}` should be {style}");
    // `_` has no words to suggest.
    if !suggestion.is_empty() {
        message.push_str(&format!(", like `{suggestion}`"));
    }
    out.push(Diagnostic::warning(source, span.clone(), message));
}

/// The PascalCase spelling of `name` to suggest. A SCREAMING_SNAKE_CASE
/// name, which has no word boundaries but its underscores, gets each word
/// capitalized (`MAX_LEN` as `MaxLen`); [`case::pascal`] would keep every
/// word as an acronym and suggest `MAXLEN`.
fn pascal_suggestion(name: &str) -> String {
    if !name.contains('_') || name.bytes().any(|byte| byte.is_ascii_lowercase()) {
        return case::pascal(name);
    }
    case::pascal(&name.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use crate::compile;

    const EXAMPLE1: &str = include_str!("../tests/data/example1.uju");
    const EXAMPLE2: &str = include_str!("../tests/data/example2.uju");

    fn warnings(sources: &[&str]) -> Vec<String> {
        compile(sources)
            .unwrap_or_else(|errors| panic!("unexpected errors: {errors:#?}"))
            .warnings
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect()
    }

    #[test]
    fn conventional_names_are_quiet() {
        assert!(warnings(&[EXAMPLE1, EXAMPLE2]).is_empty());
    }

    #[test]
    fn every_kind_of_name_is_checked() {
        let warnings = warnings(&["namespace Game.my_ns\n\
             enum color { light_red }\n\
             struct Foo_Bar { HitPoints: u8 }\n\
             message[in] spawn { struct inner { _: u8 } const max_len: u8 = 1 }\n\
             const HTTP_PORT: u16 = 80\n\
             type party_id = u64\n\
             union op { add_item { SlotIndex: u8 }, remove(u8) }\n"]);
        assert_eq!(
            warnings,
            [
                "namespace `Game` should be snake_case, like `game`",
                "enum `color` should be PascalCase, like `Color`",
                "variant `light_red` should be PascalCase, like `LightRed`",
                "struct `Foo_Bar` should be PascalCase, like `FooBar`",
                "field `HitPoints` should be snake_case, like `hit_points`",
                "message `spawn` should be PascalCase, like `Spawn`",
                "struct `inner` should be PascalCase, like `Inner`",
                "field `_` should be snake_case",
                "const `max_len` should be PascalCase, like `MaxLen`",
                "const `HTTP_PORT` should be PascalCase, like `HttpPort`",
                "type `party_id` should be PascalCase, like `PartyId`",
                "union `op` should be PascalCase, like `Op`",
                "variant `add_item` should be PascalCase, like `AddItem`",
                "field `SlotIndex` should be snake_case, like `slot_index`",
                "variant `remove` should be PascalCase, like `Remove`",
            ]
        );
    }

    #[test]
    fn warnings_accompany_errors() {
        let diagnostics = compile(&["namespace a\nstruct s { x: Missing }\n"]).unwrap_err();
        assert!(diagnostics.iter().any(|d| d.is_error()));
        assert!(
            diagnostics
                .iter()
                .any(|d| !d.is_error() && d.message.starts_with("struct `s`"))
        );
    }
}
