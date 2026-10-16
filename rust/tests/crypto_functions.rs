// cpeak::crypto: random bytes, a hash, and bytes written as text. The expected values come
// from the crypto module of Node.js, which these have to match.

use cpeak::crypto::{base64url_encode, hex_encode, random_bytes, sha256};

#[test]
fn should_give_as_many_random_bytes_as_asked_for() {
    assert_eq!(random_bytes(0).len(), 0);
    assert_eq!(random_bytes(16).len(), 16);
    assert_eq!(random_bytes(1000).len(), 1000);
}

#[test]
fn should_give_other_random_bytes_every_time() {
    assert_ne!(random_bytes(16), random_bytes(16));
}

#[test]
fn should_hash_with_sha256() {
    // crypto.createHash("sha256").update(text).digest("hex")
    assert_eq!(
        hex_encode(sha256("")),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        hex_encode(sha256("abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        hex_encode(sha256("héllo")),
        "3c48591d8d098a4538f5e013dfcf406e948eac4d3277b10bf614e295d6068179"
    );
}

#[test]
fn should_hash_a_string_and_its_bytes_the_same() {
    assert_eq!(sha256("héllo"), sha256(String::from("héllo")));
    assert_eq!(sha256("héllo"), sha256("héllo".as_bytes()));
}

#[test]
fn should_write_bytes_as_hex() {
    assert_eq!(hex_encode(""), "");
    assert_eq!(hex_encode([0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
    assert_eq!(hex_encode("Cpeak"), "437065616b");
    assert_eq!(hex_encode(random_bytes(16)).len(), 32);
}

#[test]
fn should_write_bytes_as_base64url_without_padding() {
    // The test vectors of RFC 4648, without the "=" at the end
    let vectors = [
        ("", ""),
        ("f", "Zg"),
        ("fo", "Zm8"),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg"),
        ("fooba", "Zm9vYmE"),
        ("foobar", "Zm9vYmFy"),
    ];
    for (text, encoded) in vectors {
        assert_eq!(base64url_encode(text), encoded);
    }

    // - and _ where base64 has + and /
    assert_eq!(base64url_encode([0xfb, 0xff, 0xfe]), "-__-");
    assert_eq!(base64url_encode(random_bytes(7)).len(), 10);
}
