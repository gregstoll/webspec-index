//! Lowercase hex without per-byte formatting.
pub(crate) fn encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize]);
        out.push(DIGITS[(byte & 0x0f) as usize]);
    }
    String::from_utf8(out).expect("hex digits are ASCII")
}

#[cfg(test)]
mod tests {
    #[test]
    fn encodes_lowercase_pairs() {
        assert_eq!(super::encode(&[0x00, 0x0f, 0xab, 0xff]), "000fabff");
        assert_eq!(super::encode(&[]), "");
    }
}
