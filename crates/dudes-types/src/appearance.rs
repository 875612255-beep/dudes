//! The authored cast and accent choices recorded when an agent is hired.

use core::{fmt, str::FromStr};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// An authored look in the Floor's voxel cast.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CastCharacter {
    /// The `c` choice.
    C,
    /// The `d` choice.
    D,
    /// The `e` choice.
    E,
    /// The `f` choice.
    F,
    /// The `g` choice.
    G,
    /// The `h` choice.
    H,
    /// The `i` choice.
    I,
    /// The `j` choice.
    J,
    /// The `k` choice.
    K,
    /// The `l` choice.
    L,
}

impl CastCharacter {
    /// Every supported choice in display order.
    pub const ALL: [Self; 10] = [
        Self::C,
        Self::D,
        Self::E,
        Self::F,
        Self::G,
        Self::H,
        Self::I,
        Self::J,
        Self::K,
        Self::L,
    ];

    /// The name used on the wire and command line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::C => "c",
            Self::D => "d",
            Self::E => "e",
            Self::F => "f",
            Self::G => "g",
            Self::H => "h",
            Self::I => "i",
            Self::J => "j",
            Self::K => "k",
            Self::L => "l",
        }
    }
}

impl fmt::Display for CastCharacter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Text that names no supported choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error)]
#[error("unknown cast character")]
pub struct ParseCastCharacterError;

impl FromStr for CastCharacter {
    type Err = ParseCastCharacterError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|choice| choice.as_str() == text)
            .ok_or(ParseCastCharacterError)
    }
}

/// A named accent from the Floor palette.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AccentColour {
    /// The `amber` choice.
    Amber,
    /// The `coral` choice.
    Coral,
    /// The `mint` choice.
    Mint,
    /// The `sky` choice.
    Sky,
    /// The `violet` choice.
    Violet,
    /// The `rose` choice.
    Rose,
}

impl AccentColour {
    /// Every supported choice in display order.
    pub const ALL: [Self; 6] = [
        Self::Amber,
        Self::Coral,
        Self::Mint,
        Self::Sky,
        Self::Violet,
        Self::Rose,
    ];

    /// The name used on the wire and command line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Amber => "amber",
            Self::Coral => "coral",
            Self::Mint => "mint",
            Self::Sky => "sky",
            Self::Violet => "violet",
            Self::Rose => "rose",
        }
    }
}

impl fmt::Display for AccentColour {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Text that names no supported choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error)]
#[error("unknown accent colour")]
pub struct ParseAccentColourError;

impl FromStr for AccentColour {
    type Err = ParseAccentColourError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|choice| choice.as_str() == text)
            .ok_or(ParseAccentColourError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cast_matches_the_authored_looks() {
        let looks: serde_json::Value =
            serde_json::from_str(include_str!("../../../floor/src/renderer/looks.json")).unwrap();
        assert_eq!(
            serde_json::to_value(CastCharacter::ALL).unwrap(),
            looks["characters"]
        );
    }

    #[test]
    fn finite_choices_round_trip_and_refuse_unknowns() {
        for choice in CastCharacter::ALL {
            assert_eq!(choice.as_str().parse::<CastCharacter>().unwrap(), choice);
            assert_eq!(
                serde_json::from_str::<CastCharacter>(&serde_json::to_string(&choice).unwrap())
                    .unwrap(),
                choice
            );
        }
        for choice in AccentColour::ALL {
            assert_eq!(choice.as_str().parse::<AccentColour>().unwrap(), choice);
            assert_eq!(
                serde_json::from_str::<AccentColour>(&serde_json::to_string(&choice).unwrap())
                    .unwrap(),
                choice
            );
        }
        for unknown in ["", "automatic", "a", "../cast", "C"] {
            assert!(unknown.parse::<CastCharacter>().is_err());
            assert!(serde_json::from_value::<CastCharacter>(serde_json::json!(unknown)).is_err());
        }
        for unknown in ["", "automatic", "#fff", "red", "Sky"] {
            assert!(unknown.parse::<AccentColour>().is_err());
            assert!(serde_json::from_value::<AccentColour>(serde_json::json!(unknown)).is_err());
        }
    }
}
