//! Room-code rules (spec "Server Room Management"): 6-character codes from
//! an unambiguous alphabet, readable when typed by hand.

/// 32-character unambiguous alphabet: A-Z without I and O, digits 2-9
/// (no 0/1, no I/O) so codes stay readable when typed by hand.
pub const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
pub const CODE_LEN: usize = 6;

/// A room code is valid when it is exactly 6 characters, each from the
/// unambiguous alphabet (spec scenario "Unknown or malformed room codes are
/// rejected").
pub fn is_valid_code(code: &str) -> bool {
    code.len() == CODE_LEN && code.bytes().all(|b| CODE_ALPHABET.contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_formed_codes_are_valid() {
        for code in ["ABCDEF", "Z98765", "K2L4M6"] {
            assert!(is_valid_code(code), "`{code}` should be valid");
        }
    }

    #[test]
    fn ambiguous_or_misshapen_codes_are_invalid() {
        for code in ["", "ABCDE", "ABCDEFG", "ABCDEF1", "ABCDEO", "ABCDEF0", "abcdef"] {
            assert!(!is_valid_code(code), "`{code}` should be invalid");
        }
    }
}
