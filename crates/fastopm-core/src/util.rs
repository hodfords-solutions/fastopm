//! Small helpers.

/// `n` random bytes as lower-case hex (`cryptoUtils.generateRandomHexString`): 2·n characters.
///
/// Used for `_rev` suffixes (8 bytes) and generated `server_id` values (6 bytes).
pub fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).expect("the operating system random source is available");
    hex::encode(buf)
}

#[cfg(test)]
mod tests {
    #[test]
    fn random_hex_len_and_charset() {
        let a = super::random_hex(6);
        assert_eq!(a.len(), 12);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(super::random_hex(8), super::random_hex(8));
    }
}
