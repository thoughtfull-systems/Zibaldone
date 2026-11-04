//! Machine addresses for pairing, as the pairing specification defines them.

use std::fmt;
use std::net::SocketAddr;

use crate::{Error, base32};

/// A machine's 32-byte Ed25519 public key.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MachineKey(pub [u8; 32]);

impl MachineKey {
    /// The 64 lowercase hexadecimal digits that the space document stores.
    pub fn to_hex(&self) -> String {
        use fmt::Write;
        self.0.iter().fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
    }

    pub fn from_hex(text: &str) -> Result<Self, Error> {
        let bytes = text.as_bytes();
        if bytes.len() != 64 {
            return Err(Error::invalid("machine key", text));
        }
        let mut out = [0u8; 32];
        for (i, pair) in bytes.chunks(2).enumerate() {
            let pair =
                std::str::from_utf8(pair).map_err(|_| Error::invalid("machine key", text))?;
            out[i] =
                u8::from_str_radix(pair, 16).map_err(|_| Error::invalid("machine key", text))?;
        }
        Ok(Self(out))
    }

    /// The 52 lowercase Base32 digits, without hyphens, as invites hold them.
    pub fn to_base32(&self) -> String {
        base32::encode(&self.0, 52)
    }

    /// The form apps show: uppercase, in 13 groups of 4 separated by hyphens.
    pub fn to_display(&self) -> String {
        let digits = self.to_base32().to_ascii_uppercase();
        digits
            .as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect::<Vec<_>>()
            .join("-")
    }
}

impl fmt::Debug for MachineKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MachineKey({})", self.to_display())
    }
}

/// A machine's address: its key, and network addresses to try before looking the key up.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Address {
    pub key: MachineKey,
    pub hints: Vec<SocketAddr>,
}

impl Address {
    /// Reads an address that someone typed or pasted.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let (key, hints) = match text.split_once('@') {
            Some((key, hints)) => (key, Some(hints)),
            None => (text, None),
        };
        let key = base32::decode::<32>(key, 52, true)
            .map(MachineKey)
            .ok_or_else(|| Error::invalid("machine address", text))?;
        let hints = match hints {
            None => Vec::new(),
            Some(hints) => hints
                .split(',')
                .map(|hint| {
                    hint.trim().parse().map_err(|_| Error::invalid("network address", hint))
                })
                .collect::<Result<_, _>>()?,
        };
        Ok(Self { key, hints })
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key.to_display())?;
        for (i, hint) in self.hints.iter().enumerate() {
            f.write_str(if i == 0 { "@" } else { "," })?;
            write!(f, "{hint}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_examples_in_the_specification() {
        let keys = [[0u8; 32], std::array::from_fn(|i| u8::try_from(i).unwrap()), [0xff; 32]];
        let texts = [
            "0000-0000-0000-0000-0000-0000-0000-0000-0000-0000-0000-0000-0000",
            "0001-081G-8186-0W40-J2GB-1G6G-W3RG-2491-650N-2RBH-G68T-3CE1-T7GZ",
            "1ZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZ",
        ];
        for (key, text) in keys.iter().zip(texts) {
            assert_eq!(MachineKey(*key).to_display(), text);
            assert_eq!(Address::parse(text).unwrap().key, MachineKey(*key));
        }
    }

    #[test]
    fn reads_hints_and_forgives_typing() {
        let text = "0001 081g-8186-0w40-j2gb-1g6g-w3rg-2491-650n-2rbh-g68t-3ce1-t7gz\
                    @192.168.1.20:41641,[::1]:7000";
        let address = Address::parse(text).unwrap();
        assert_eq!(address.hints.len(), 2);
        assert_eq!(Address::parse(&address.to_string()).unwrap(), address);
    }

    #[test]
    fn round_trips_hex() {
        let key = MachineKey(std::array::from_fn(|i| u8::try_from(i * 7).unwrap()));
        assert_eq!(MachineKey::from_hex(&key.to_hex()).unwrap(), key);
    }
}
