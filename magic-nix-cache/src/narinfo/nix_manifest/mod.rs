//! The Nix manifest format.
//!
//! Nix uses a simple format in binary cache manifests (`.narinfo`,
//! `/nix-cache-info`). It consists of a single, flat KV map with
//! colon (`:`) as the delimiter.
//!
//! It's not well-defined and the official implementation performs
//! serialization and deserialization by hand [1]. Here we implement
//! a deserializer and a serializer using the serde framework.
//!
//! An example of a `/nix-cache-info` file:
//!
//! ```text
//! StoreDir: /nix/store
//! WantMassQuery: 1
//! Priority: 40
//! ```
//!
//! [1] <https://github.com/NixOS/nix/blob/d581129ef9ef5d7d65e676f6a7bfe36c82f6ea6e/src/libstore/nar-info.cc#L28>

mod serializer;

use std::fmt::Display;
use std::result::Result as StdResult;

use serde::{de, ser, Serialize};
use serde_with::{formats::SpaceSeparator, StringWithSeparator};

use serializer::Serializer;

type Result<T> = StdResult<T, Error>;

/// Custom (de)serializer for a space-delimited list.
///
/// Example usage:
///
/// ```ignore
/// use serde::Deserialize;
/// use serde_with::serde_as;
/// # use attic_server::nix_manifest::{self, SpaceDelimitedList};
///
/// #[serde_as]
/// #[derive(Debug, Deserialize)]
/// struct MyManifest {
///     #[serde_as(as = "SpaceDelimitedList")]
///     some_list: Vec<String>,
/// }
///
/// let s = "some_list: item-a item-b";
/// let parsed: MyManifest = nix_manifest::from_str(s).unwrap();
///
/// assert_eq!(vec![ "item-a", "item-b" ], parsed.some_list);
/// ```
pub type SpaceDelimitedList = StringWithSeparator<SpaceSeparator, String>;

pub fn to_string<T>(value: &T) -> String
where
    T: Serialize,
{
    let mut serializer = Serializer::new();
    value
        .serialize(&mut serializer)
        .expect("failed to convert path into to nar info");

    serializer.into_output()
}

/// An error during (de)serialization.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// "{0}" values are unsupported.
    Unsupported(&'static str),

    /// None is unsupported. Add #[serde(skip_serializing_if = "Option::is_none")]
    NoneUnsupported,

    /// Nested maps are unsupported.
    NestedMapUnsupported,

    /// Custom error: {0}
    Custom(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        match self {
            Error::Unsupported(s) => f.write_fmt(format_args!(r#""{s}" values are unsupported."#)),
            Error::NoneUnsupported => f.write_str(
                r#""None is unsupported. Add #[serde(skip_serializing_if = "Option::is_none")]""#,
            ),
            Error::NestedMapUnsupported => f.write_str("Nexted maps are unsupported."),
            Error::Custom(s) => f.write_fmt(format_args!(r#"Custom error: {s}"#)),
        }
    }
}

impl de::Error for Error {
    fn custom<T: Display>(msg: T) -> Self {
        let f = format!("{}", msg);
        Self::Custom(f)
    }
}

impl ser::Error for Error {
    fn custom<T: Display>(msg: T) -> Self {
        let f = format!("{}", msg);
        Self::Custom(f)
    }
}
