//! ujuc's options, as read from `ujuc.toml`. Parsing takes the file's text;
//! finding and reading the file is the caller's job.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

/// Every option `ujuc.toml` sets. Each section and field is optional, and
/// an unknown one is an error.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The `[rust]` section, for the Rust backend.
    pub rust: RustConfig,
}

/// The `[rust]` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RustConfig {
    /// Foreign types, from `[rust.map]`: each schema declaration's qualified
    /// path, such as `uju.math.Vec3`, to the Rust type stored in its place,
    /// such as `::glam::Vec3`. The mapping is not part of the protocol.
    pub map: BTreeMap<String, String>,
}

impl Config {
    /// Parse the text of a `ujuc.toml`. Only what holds without a schema is
    /// checked here: whether a mapped path names a declaration is the
    /// backend's to check.
    pub fn parse(text: &str) -> Result<Config, Error> {
        let config: Config = toml::from_str(text).map_err(Error::Toml)?;
        for (path, ty) in &config.rust.map {
            if path.trim().is_empty() {
                return Err(Error::EmptyPath);
            }
            if ty.trim().is_empty() {
                return Err(Error::EmptyType(path.clone()));
            }
        }
        Ok(config)
    }
}

/// Why a `ujuc.toml` was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The text is not TOML, or doesn't match [`Config`]'s shape.
    Toml(toml::de::Error),
    /// `[rust.map]` maps an empty schema path.
    EmptyPath,
    /// `[rust.map]` maps this schema path to an empty Rust type.
    EmptyType(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Toml(error) => write!(f, "{}", error.to_string().trim_end()),
            Error::EmptyPath => f.write_str("`[rust.map]` maps an empty schema path"),
            Error::EmptyType(path) => {
                write!(f, "`[rust.map]` maps `{path}` to an empty Rust type")
            }
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_default() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
        assert_eq!(Config::parse("[rust]\n").unwrap(), Config::default());
        assert_eq!(Config::parse("[rust.map]\n").unwrap(), Config::default());
    }

    #[test]
    fn map() {
        let config = Config::parse(
            r#"
            [rust.map]
            "uju.math.Vec3" = "::glam::Vec3"
            "foo.Id" = "crate::Id"
            "#,
        )
        .unwrap();
        let expected = BTreeMap::from([
            ("uju.math.Vec3".to_owned(), "::glam::Vec3".to_owned()),
            ("foo.Id".to_owned(), "crate::Id".to_owned()),
        ]);
        assert_eq!(config.rust.map, expected);
    }

    #[test]
    fn example() {
        let config = Config::parse(include_str!("../tests/data/ujuc.toml")).unwrap();
        assert_eq!(config.rust.map["foo.bar.Position"], "::glam::Vec2");
        assert_eq!(config.rust.map["foo.bar.GuildId"], "crate::guild::GuildId");
    }

    #[test]
    fn unknown_rejected() {
        for text in [
            "[rsut.map]\n",
            "[rust.mpa]\n",
            "[rust]\nmap = {}\nextra = 1\n",
            "top = 1\n",
        ] {
            let error = Config::parse(text).unwrap_err();
            assert!(matches!(error, Error::Toml(_)), "{text:?}: {error}");
            assert!(error.to_string().contains("unknown field"), "{error}");
        }
    }

    #[test]
    fn wrong_type_rejected() {
        let error = Config::parse("[rust.map]\n\"a.B\" = 3\n").unwrap_err();
        assert!(matches!(error, Error::Toml(_)), "{error}");
    }

    #[test]
    fn empty_path_rejected() {
        let error = Config::parse("[rust.map]\n\"\" = \"::glam::Vec3\"\n").unwrap_err();
        assert_eq!(error, Error::EmptyPath);
        assert_eq!(error.to_string(), "`[rust.map]` maps an empty schema path");
    }

    #[test]
    fn empty_type_rejected() {
        let error = Config::parse("[rust.map]\n\"uju.math.Vec3\" = \" \"\n").unwrap_err();
        assert_eq!(error, Error::EmptyType("uju.math.Vec3".to_owned()));
        assert_eq!(
            error.to_string(),
            "`[rust.map]` maps `uju.math.Vec3` to an empty Rust type"
        );
    }
}
