//! Field bounds + validators (edge-cases/limits.md), exposed as typed helpers
//! so both server and client validate identically (BUILD-PLAN T1-13).

/// Username: 3–20 chars, `^[a-zA-Z0-9_]+$` (CANON.md D17, limits.md).
pub const USERNAME_MIN: usize = 3;
pub const USERNAME_MAX: usize = 20;
/// Password: 8–128 chars (CANON.md D17).
pub const PASSWORD_MIN: usize = 8;
pub const PASSWORD_MAX: usize = 128;
/// Party size (limits.md, CANON.md D13).
pub const PARTY_MAX: usize = 4;

/// Returns `true` when `name` satisfies the username contract.
pub fn is_valid_username(name: &str) -> bool {
    let len = name.chars().count();
    (USERNAME_MIN..=USERNAME_MAX).contains(&len)
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Returns `true` when `pw` satisfies the password length contract.
pub fn is_valid_password(pw: &str) -> bool {
    let len = pw.chars().count();
    (PASSWORD_MIN..=PASSWORD_MAX).contains(&len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn username_rules() {
        assert!(is_valid_username("MazeRunner_88"));
        assert!(!is_valid_username("ab")); // too short
        assert!(!is_valid_username("has space"));
        assert!(!is_valid_username("emoji😀"));
        assert!(!is_valid_username(&"x".repeat(21))); // too long
    }

    #[test]
    fn password_rules() {
        assert!(is_valid_password("correct-horse"));
        assert!(!is_valid_password("short"));
    }
}
