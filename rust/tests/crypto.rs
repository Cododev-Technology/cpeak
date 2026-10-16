// The crypto library of the program: Cpeak picks it, so the crates that use rustls don't have to.
// (it's picked once for the whole program, which is why this file has only one test)

use cpeak::cpeak;
use rustls::crypto::CryptoProvider;

#[test]
fn should_make_ring_the_crypto_library_of_the_program() {
    assert!(CryptoProvider::get_default().is_none());

    let _server = cpeak();

    assert!(CryptoProvider::get_default().is_some());

    // What a crate that leaves the choice to the program does (an HTTP client, a database
    // client, ...). Without a crypto library, this panics.
    let _config = rustls::ClientConfig::builder()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
}
