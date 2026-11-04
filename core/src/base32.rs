//! Crockford's Base32, as the identifiers specification uses it for identifiers and the pairing
//! specification for machine addresses.

/// The 32 digits, for the values 0 to 31 in order.
pub const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Writes `bytes` as `digits` digits, from the highest 5 bits to the lowest, with zero bits added
/// beyond the highest bit to fill the first digit.
pub fn encode(bytes: &[u8], digits: usize) -> String {
    debug_assert!(digits * 5 >= bytes.len() * 8 && digits * 5 < bytes.len() * 8 + 5);
    let mut out = vec![b'0'; digits];
    let mut acc: u32 = 0;
    let mut bits = 0;
    let mut position = digits;
    for &byte in bytes.iter().rev() {
        acc |= u32::from(byte) << bits;
        bits += 8;
        while bits >= 5 {
            position -= 1;
            out[position] = ALPHABET[(acc & 31) as usize];
            acc >>= 5;
            bits -= 5;
        }
    }
    if bits > 0 {
        position -= 1;
        out[position] = ALPHABET[(acc & 31) as usize];
    }
    String::from_utf8(out).expect("the alphabet is ASCII")
}

/// Reads text that people may have copied or typed: it removes hyphens, and white space when
/// `allow_space` is set, turns letters to lowercase, and reads `i` and `l` as `1` and `o` as `0`.
/// It returns `None` unless exactly `N` bytes' worth of digits remain, with the padding bits zero.
pub fn decode<const N: usize>(text: &str, digits: usize, allow_space: bool) -> Option<[u8; N]> {
    let mut values = Vec::with_capacity(digits);
    for c in text.chars() {
        if c == '-' || (allow_space && c.is_whitespace()) {
            continue;
        }
        let c = match c.to_ascii_lowercase() {
            'i' | 'l' => '1',
            'o' => '0',
            other => other,
        };
        let value = ALPHABET.iter().position(|&d| char::from(d) == c)?;
        values.push(u8::try_from(value).expect("an index below 32"));
    }
    if values.len() != digits {
        return None;
    }
    let mut out = [0u8; N];
    let mut acc: u32 = 0;
    let mut bits = 0;
    let mut position = N;
    for &value in values.iter().rev() {
        acc |= u32::from(value) << bits;
        bits += 5;
        if bits >= 8 {
            if position == 0 {
                return None;
            }
            position -= 1;
            out[position] = (acc & 0xff) as u8;
            acc >>= 8;
            bits -= 8;
        }
    }
    // What remains is the padding beyond the highest bit, which must be zero.
    (position == 0 && acc == 0).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let bytes: [u8; 16] = std::array::from_fn(|i| u8::try_from(i * 17).unwrap());
        let text = encode(&bytes, 26);
        assert_eq!(decode::<16>(&text, 26, false), Some(bytes));
    }

    #[test]
    fn rejects_padding_bits() {
        assert_eq!(decode::<16>("80000000000000000000000000", 26, false), None);
    }
}
