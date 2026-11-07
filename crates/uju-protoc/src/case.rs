//! Case conversion between schema names and each backend's conventions.
//!
//! A name is split into [`words`] once, then joined in the style a backend
//! wants. The rules are ours rather than a crate's: generated names are API,
//! so they change only when these functions (and their tests) do.
//!
//! Schema identifiers are ASCII (`[A-Za-z_][A-Za-z0-9_]*`), so case is only
//! ever ASCII case.

/// `name` split into words: at underscores, before an uppercase letter that
/// follows a lowercase letter or a digit, and before the last capital of an
/// acronym followed by lowercase. Digits stay with the word before them.
///
/// `HTTPDetail` is `HTTP Detail`, `hp_max` is `hp max`, `Vec2D` is
/// `Vec2 D`, and `player2_id` is `player2 id`. Repeated, leading, and
/// trailing underscores produce no empty words.
pub fn words(name: &str) -> Vec<&str> {
    let bytes = name.as_bytes();
    let mut words = Vec::new();
    let mut start = 0;
    for (index, &byte) in bytes.iter().enumerate() {
        if byte == b'_' {
            if start < index {
                words.push(&name[start..index]);
            }
            start = index + 1;
        } else if byte.is_ascii_uppercase() && start < index {
            let previous = bytes[index - 1];
            let next = bytes.get(index + 1);
            if previous.is_ascii_lowercase()
                || previous.is_ascii_digit()
                || (previous.is_ascii_uppercase() && next.is_some_and(u8::is_ascii_lowercase))
            {
                words.push(&name[start..index]);
                start = index;
            }
        }
    }
    if start < name.len() {
        words.push(&name[start..]);
    }
    words
}

/// `snake_case`: `HTTPDetail` becomes `http_detail`.
pub fn snake(name: &str) -> String {
    words(name)
        .iter()
        .map(|word| word.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("_")
}

/// `SCREAMING_SNAKE_CASE`: `HTTPDetail` becomes `HTTP_DETAIL`.
pub fn shouty(name: &str) -> String {
    words(name)
        .iter()
        .map(|word| word.to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join("_")
}

/// `PascalCase`, capitalizing each word and keeping the rest as written, so
/// an acronym survives: `my_ns` becomes `MyNs`, and `HTTPDetail` stays.
pub fn pascal(name: &str) -> String {
    words(name).iter().map(|word| capitalize(word)).collect()
}

/// `camelCase`: the first word lowercased, the rest as in [`pascal`], so
/// `HTTPDetail` becomes `httpDetail` and `hp_max` becomes `hpMax`.
pub fn camel(name: &str) -> String {
    let words = words(name);
    let Some((first, rest)) = words.split_first() else {
        return String::new();
    };
    let mut out = first.to_ascii_lowercase();
    for word in rest {
        out.push_str(&capitalize(word));
    }
    out
}

/// Whether `name` is already `snake_case`.
pub fn is_snake(name: &str) -> bool {
    snake(name) == name
}

/// Whether `name` is already `PascalCase`, acronyms included.
pub fn is_pascal(name: &str) -> bool {
    pascal(name) == name
}

fn capitalize(word: &str) -> String {
    let mut out = word.to_string();
    if let Some(first) = out.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_split_at_case_and_underscores() {
        let cases: [(&str, &[&str]); 11] = [
            ("MyResponse", &["My", "Response"]),
            ("HTTPDetail", &["HTTP", "Detail"]),
            ("HTTP2Server", &["HTTP2", "Server"]),
            ("IOError", &["IO", "Error"]),
            ("Vec2D", &["Vec2", "D"]),
            ("ABC", &["ABC"]),
            ("hp_max", &["hp", "max"]),
            ("player2_id", &["player2", "id"]),
            ("get_URL", &["get", "URL"]),
            ("Foo_Bar", &["Foo", "Bar"]),
            ("__a__b_", &["a", "b"]),
        ];
        for (name, expected) in cases {
            assert_eq!(words(name), expected, "{name}");
        }
        assert!(words("").is_empty());
        assert!(words("_").is_empty());
    }

    #[test]
    fn styles() {
        let cases = [
            // name, snake, shouty, pascal, camel
            (
                "MyResponse",
                "my_response",
                "MY_RESPONSE",
                "MyResponse",
                "myResponse",
            ),
            (
                "HTTPDetail",
                "http_detail",
                "HTTP_DETAIL",
                "HTTPDetail",
                "httpDetail",
            ),
            ("IOError", "io_error", "IO_ERROR", "IOError", "ioError"),
            ("Vec2D", "vec2_d", "VEC2_D", "Vec2D", "vec2D"),
            ("hp_max", "hp_max", "HP_MAX", "HpMax", "hpMax"),
            ("my_ns", "my_ns", "MY_NS", "MyNs", "myNs"),
            ("get_URL", "get_url", "GET_URL", "GetURL", "getURL"),
            ("Type", "type", "TYPE", "Type", "type"),
        ];
        for (name, snake_, shouty_, pascal_, camel_) in cases {
            assert_eq!(snake(name), snake_, "snake {name}");
            assert_eq!(shouty(name), shouty_, "shouty {name}");
            assert_eq!(pascal(name), pascal_, "pascal {name}");
            assert_eq!(camel(name), camel_, "camel {name}");
        }
    }

    #[test]
    fn styles_recognize_themselves() {
        for name in ["my_response", "hp_max", "vec2_d", "a"] {
            assert!(is_snake(name), "{name}");
        }
        for name in ["MyResponse", "HP", "hpMax", "Hp_max", "hp__max", "_hp"] {
            assert!(!is_snake(name), "{name}");
        }
        for name in ["MyResponse", "HTTPDetail", "IOError", "Vec2D", "A"] {
            assert!(is_pascal(name), "{name}");
        }
        for name in ["myResponse", "My_Response", "my_response", "Foo_"] {
            assert!(!is_pascal(name), "{name}");
        }
    }
}
