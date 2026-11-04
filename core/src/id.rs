//! Identifiers of notes, spaces, and machines, as the identifiers specification defines them.

use std::fmt;
use std::str::FromStr;

use crate::{Error, base32};

/// A version 7 UUID, which identifies a note, a space, or a machine.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Id([u8; 16]);

impl Id {
    /// A new identifier, which sorts after every identifier this process made before it.
    pub fn generate() -> Self {
        Self(*uuid::Uuid::now_v7().as_bytes())
    }

    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Reads any form of an identifier that people might copy or type, as the identifiers
    /// specification's reading rules allow.
    pub fn parse(text: &str) -> Result<Self, Error> {
        base32::decode::<16>(text, 26, false)
            .map(Self)
            .ok_or_else(|| Error::invalid("identifier", text))
    }
}

/// Writes the canonical form: 26 lowercase digits.
impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&base32::encode(&self.0, 26))
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({self})")
    }
}

impl FromStr for Id {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Error> {
        Self::parse(text)
    }
}

impl serde::Serialize for Id {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Id {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> [u8; 16] {
        std::array::from_fn(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap())
    }

    #[test]
    fn matches_the_examples_in_the_specification() {
        for (bytes, text) in [
            ("00000000000000000000000000000000", "00000000000000000000000000"),
            ("ffffffffffffffffffffffffffffffff", "7zzzzzzzzzzzzzzzzzzzzzzzzz"),
            ("017f22e279b07cc398c4dc0c0c07398f", "01fwhe4ydgfk1shh6w1g60eecf"),
            ("019a4f3c7e217b3a9c4d2f8e61a0b5c7", "01k97krzh1fcx9rk9fhsgt1de7"),
        ] {
            let id = Id::from_bytes(hex(bytes));
            assert_eq!(id.to_string(), text);
            assert_eq!(Id::parse(text).unwrap(), id);
        }
    }

    #[test]
    fn reads_what_people_type() {
        let id = Id::parse("01k97krzh1fcx9rk9fhsgt1de7").unwrap();
        assert_eq!(Id::parse("01K97-KRZH1-FCX9R-K9FHS-GTIDE7").unwrap(), id);
        assert_eq!(Id::parse("0Lk97krzh1fcx9rk9fhsgt1de7").unwrap(), id);
    }

    #[test]
    fn rejects_bad_text() {
        for text in [
            "",
            "01k97krzh1fcx9rk9fhsgt1de",
            "81k97krzh1fcx9rk9fhsgt1de7",
            "01k97krzh1fcx9rk9fhsgt1deu",
        ] {
            assert!(Id::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn sorts_by_creation() {
        let ids: Vec<Id> = (0..100).map(|_| Id::generate()).collect();
        let texts: Vec<String> = ids.iter().map(ToString::to_string).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]));
        assert!(texts.windows(2).all(|w| w[0] < w[1]));
    }
}
