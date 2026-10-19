//! Password hashing with Argon2id, and the rules for usernames and passwords.
//!
//! Argon2id is the current recommendation (OWASP, RFC 9106) for storing
//! passwords: it is deliberately slow and needs a lot of memory per guess, so
//! a stolen hash is very expensive to brute-force, even on GPUs. We use the
//! `argon2` crate's defaults, which match OWASP's minimum: 19 MiB, 2 passes,
//! 1 lane. Each hash gets its own random salt, and the result is a PHC string
//! (`$argon2id$v=19$m=19456,t=2,p=1$salt$hash`) that records the parameters,
//! so they can be raised later without breaking existing hashes.

use std::sync::OnceLock;

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier},
};

pub const MIN_PASSWORD_CHARS: usize = 12;
/// Longer input is refused before hashing, so nobody can make us hash megabytes.
pub const MAX_PASSWORD_CHARS: usize = 256;

/// Hashes a new password (slow: tens of milliseconds and 19 MiB of memory).
/// Call it from a blocking thread, see `Auth`.
pub fn hash(password: &str) -> anyhow::Result<String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("hashing the password failed: {e}"))
}

/// Checks `password` against a stored PHC string. Wrong password, a corrupt
/// hash: both are simply "no".
pub fn verify(password: &str, phc: &str) -> bool {
    Argon2::default()
        .verify_password(password.as_bytes(), phc)
        .is_ok()
}

/// A hash of a random password nobody knows. Logins for unknown usernames are
/// checked against it, so they take as long as real ones and the response
/// time doesn't reveal which usernames exist.
pub fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("the OS random number generator failed");
        let random: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        hash(&random).expect("hashing a random password")
    })
}

/// Usernames are case-insensitive: stored and compared in lower case.
pub fn normalize_username(name: &str) -> String {
    name.trim().to_lowercase()
}

/// 1–32 characters: `a-z`, `0-9`, `.`, `_`, `-`, starting with a letter or
/// digit (the same rule as the database's CHECK constraint).
pub fn check_username(name: &str) -> Result<(), String> {
    let ok = (1..=32).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        });
    if ok {
        Ok(())
    } else {
        Err("username must be 1-32 characters: a-z, 0-9, '.', '_' or '-', starting with a letter or digit".into())
    }
}

/// Length is what matters most (NIST SP 800-63B): at least 12 characters, no
/// composition rules. Use a password manager or a few random words.
pub fn check_password(password: &str) -> Result<(), String> {
    let n = password.chars().count();
    if n < MIN_PASSWORD_CHARS {
        return Err(format!(
            "password must be at least {MIN_PASSWORD_CHARS} characters"
        ));
    }
    if n > MAX_PASSWORD_CHARS {
        return Err(format!(
            "password must be at most {MAX_PASSWORD_CHARS} characters"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify() {
        let h = hash("correct horse battery").unwrap();
        assert!(h.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{h}");
        assert!(verify("correct horse battery", &h));
        assert!(!verify("correct horse batterY", &h));
        assert!(!verify("correct horse battery", "not a hash"));
        // Same password, new salt, different hash.
        assert_ne!(h, hash("correct horse battery").unwrap());
        assert!(!verify("", dummy_hash()));
    }

    #[test]
    fn username_rules() {
        assert_eq!(normalize_username("  Probe "), "probe");
        for ok in ["probe", "a", "user.name_1-x", &"a".repeat(32)] {
            assert!(check_username(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-x", ".x", "Probe", "a b", "ž", &"a".repeat(33)] {
            assert!(check_username(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn password_rules() {
        assert!(check_password("short").is_err());
        assert!(check_password(&"a".repeat(12)).is_ok());
        assert!(check_password(&"ž".repeat(12)).is_ok());
        assert!(check_password(&"a".repeat(257)).is_err());
    }
}
