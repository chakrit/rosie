//! Pack and rule names (`docs/spec/rules.md#names`).

use unicode_ident::{is_xid_continue, is_xid_start};

const MAX_CHARS: usize = 64;

/// Whether `name` is a valid pack or rule name: a letter or digit first, then
/// `XID_Continue` characters, `_`, or `-`, at most 64 characters.
///
/// A valid name never starts with `.` and never holds `/`, so it is always one plain
/// path component.
pub fn is_valid(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    let starts_well = is_xid_start(first) || first.is_ascii_digit();
    let continues_well = chars.all(|c| is_xid_continue(c) || c == '-');
    let short_enough = name.chars().count() <= MAX_CHARS;
    starts_well && continues_well && short_enough
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_letters_digits_underscores_and_hyphens() {
        for name in [
            "rosie",
            "node-modules",
            "python_venv",
            "3d-tools",
            "données",
        ] {
            assert!(is_valid(name), "{name} should be valid");
        }
    }

    #[test]
    fn refuses_names_that_are_not_one_plain_component() {
        for name in [
            "", ".hidden", "a/b", "a.b", "..", "-lead", "_lead", "a b", "a🦀",
        ] {
            assert!(!is_valid(name), "{name:?} should be refused");
        }
    }

    #[test]
    fn allows_at_most_64_characters() {
        let longest = "a".repeat(64);
        let too_long = "a".repeat(65);

        assert!(is_valid(&longest));
        assert!(!is_valid(&too_long));
    }
}
