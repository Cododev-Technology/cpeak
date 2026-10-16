// The crypto library of the program: an app that picked its own before Cpeak got to it keeps it.
// (it's picked once for the whole program, which is why this file has only one test)

use cpeak::cpeak;
use rustls::crypto::{ring, CryptoProvider};

#[test]
fn should_keep_the_crypto_library_that_the_app_picked() {
    // ring as well, but told apart by its single cipher suite
    let mut own = ring::default_provider();
    own.cipher_suites.truncate(1);
    own.install_default().unwrap();

    let _server = cpeak();
    cpeak::run(async {});

    let picked = CryptoProvider::get_default().unwrap();
    assert_eq!(picked.cipher_suites.len(), 1);
}
