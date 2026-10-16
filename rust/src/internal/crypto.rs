// Node.js has the crypto module built in. These are the parts of it that almost every server
// needs (random bytes, a hash, and bytes written as text), and your app gets them as cpeak::crypto.

use sha2::{Digest, Sha256};

// Like Node.js's crypto.randomBytes(size): bytes from the secure random source of the
// operating system, so they are safe to use for tokens, ids and salts. Node.js throws when
// the system can't give any, and so does this (it panics).
pub fn random_bytes(size: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; size];
    getrandom::fill(&mut bytes)
        .unwrap_or_else(|error| panic!("Failed to generate random bytes: {error}"));
    bytes
}

// Like Node.js's crypto.createHash("sha256").update(data).digest(): the 32 bytes of the hash
pub fn sha256(data: impl AsRef<[u8]>) -> [u8; 32] {
    Sha256::digest(data).into()
}

// The same as Node.js's buffer.toString("hex")
pub fn hex_encode(bytes: impl AsRef<[u8]>) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let bytes = bytes.as_ref();
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        hex.push(HEX_DIGITS[(byte >> 4) as usize] as char);
        hex.push(HEX_DIGITS[(byte & 0xf) as usize] as char);
    }
    hex
}

// The same as Node.js's digest("base64url"): URL-safe base64, without the "=" padding
pub fn base64url_encode(bytes: impl AsRef<[u8]>) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let bytes = bytes.as_ref();
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        encoded.push(ALPHABET[b0 >> 2] as char);
        encoded.push(ALPHABET[((b0 & 0b11) << 4) | (b1 >> 4)] as char);
        if chunk.len() > 1 {
            encoded.push(ALPHABET[((b1 & 0b1111) << 2) | (b2 >> 6)] as char);
        }
        if chunk.len() > 2 {
            encoded.push(ALPHABET[b2 & 0b111111] as char);
        }
    }
    encoded
}
