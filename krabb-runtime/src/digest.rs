//! Lowercase hexadecimal text for digests, nonces, and derived identifiers.

use sha2::{Digest, Sha256};

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn sha256(data: impl AsRef<[u8]>) -> String {
    hex(&Sha256::digest(data))
}

#[cfg(test)]
mod tests {
    #[test]
    fn digests_are_lowercase_hexadecimal() {
        assert_eq!(super::hex(&[0, 171, 255]), "00abff");
        assert_eq!(
            super::sha256("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
