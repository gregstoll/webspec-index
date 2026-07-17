// ITU-T Recommendation support
//
// Handles name parsing/canonicalization, dynamic edition discovery via the
// rec.aspx listing page, and canonical free-PDF URL construction.

/// Check whether a spec name looks like an ITU-T Recommendation: one letter,
/// a dot, then one or more dot-separated digit groups (e.g. "H.265",
/// "H.265.1", "T.35", "G.711"). Case-insensitive on the leading letter.
pub fn is_itu_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
        return false;
    }
    if bytes.get(1) != Some(&b'.') {
        return false;
    }
    let rest = &name[2..];
    if rest.is_empty() {
        return false;
    }
    rest.split('.')
        .all(|group| !group.is_empty() && group.bytes().all(|b| b.is_ascii_digit()))
}

/// Convert an ITU-T Recommendation name to its canonical form: uppercase
/// leading letter, rest unchanged (it's already digits and dots).
/// "h.265" -> "H.265", "H.265" -> "H.265".
pub fn canonical_itu_name(name: &str) -> String {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    format!("{}{}", first.to_ascii_uppercase(), &name[first.len_utf8()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_itu_name_basic() {
        assert!(is_itu_name("H.265"));
        assert!(is_itu_name("h.265"));
        assert!(is_itu_name("T.35"));
        assert!(is_itu_name("G.711"));
    }

    #[test]
    fn test_is_itu_name_multi_part() {
        assert!(is_itu_name("H.265.1"));
    }

    #[test]
    fn test_is_itu_name_rejects_non_itu() {
        assert!(!is_itu_name("HTML"));
        assert!(!is_itu_name("RFC9110"));
        assert!(!is_itu_name("CSS-GRID"));
        assert!(!is_itu_name("H."));
        assert!(!is_itu_name("H.26A"));
        assert!(!is_itu_name(""));
        assert!(!is_itu_name("H"));
    }

    #[test]
    fn test_canonical_itu_name() {
        assert_eq!(canonical_itu_name("h.265"), "H.265");
        assert_eq!(canonical_itu_name("H.265"), "H.265");
        assert_eq!(canonical_itu_name("t.35"), "T.35");
    }
}
