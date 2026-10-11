use std::sync::Arc;
use std::time::{Duration, SystemTime};

use hmac::{Hmac, Mac};
use sha2::{Sha256, Sha384, Sha512};

use crate::internal::errors::{framework_error, throw, CpeakError, ErrorCode};
use crate::types::{Middleware, Next};
use crate::utils::types::{
    AuthOptions, DynFindToken, DynRevokeToken, DynSaveToken, PbkdfOptions, VerifiedToken,
};
use crate::{CpeakRequest, CpeakResponse};

pub(crate) struct Defaults {
    pub(crate) iterations: u32,
    pub(crate) keylen: usize,
    pub(crate) digest: &'static str,
    pub(crate) salt_size: usize,
    pub(crate) hmac_algorithm: &'static str,
    pub(crate) token_id_size: usize,
    pub(crate) token_expiry: u64,
}

pub(crate) const DEFAULTS: Defaults = Defaults {
    iterations: 210_000,
    keylen: 64,
    digest: "sha512",
    salt_size: 32,
    hmac_algorithm: "sha256",
    token_id_size: 20,
    token_expiry: 7 * 24 * 60 * 60 * 1000, // 7 days in ms
};

pub async fn hash_password(
    password: &str,
    options: impl Into<Option<PbkdfOptions>>,
) -> Result<String, CpeakError> {
    let options = options.into().unwrap_or_default();
    let iterations = options.iterations;
    let keylen = options.keylen;
    let digest = options.digest;
    let salt = random_bytes(options.salt_size)?;
    let hash = pbkdf2_async(password, salt.clone(), iterations, keylen, &digest).await?;
    Ok(format!(
        "pbkdf2:{iterations}:{keylen}:{digest}:{}:{}",
        hex_encode(&salt),
        hex_encode(&hash)
    ))
}

pub async fn verify_password(password: &str, stored: &str) -> Result<bool, CpeakError> {
    // When argon2 is added, dispatch on the prefix here.
    let without_prefix = stored
        .find(':')
        .map_or(stored, |index| &stored[index + 1..]);
    let parts: Vec<&str> = without_prefix.split(':').collect();
    let [iters_str, keylen_str, digest, salt_hex, hash_hex] = parts[..] else {
        return Ok(false);
    };
    let (Some(iterations), Some(keylen)) = (parse_int(iters_str), parse_int(keylen_str)) else {
        return Ok(false);
    };
    if digest.is_empty() || salt_hex.is_empty() || hash_hex.is_empty() {
        return Ok(false);
    }
    let (Ok(iterations), Ok(keylen)) = (u32::try_from(iterations), usize::try_from(keylen)) else {
        return Ok(false);
    };
    let salt = hex_decode(salt_hex);
    let stored_hash = hex_decode(hash_hex);
    // The hashes can only match if they have the same length, so we don't even bother computing it otherwise
    if stored_hash.len() != keylen {
        return Ok(false);
    }
    let hash = pbkdf2_async(password, salt, iterations, keylen, digest).await?;
    if stored_hash.len() != hash.len() {
        return Ok(false);
    }
    Ok(timing_safe_equal(&hash, &stored_hash))
}

fn sign_token(token_id: &str, secret: &str, algorithm: &str) -> Result<String, CpeakError> {
    let sig = hmac_hex(algorithm, secret, token_id)?;
    Ok(format!("{token_id}.{sig}"))
}

fn extract_token_id(token: &str, secret: &str, algorithm: &str) -> Option<String> {
    let dot = token.find('.')?;
    let token_id = &token[..dot];
    let sig = &token[dot + 1..];
    let expected = hmac_hex(algorithm, secret, token_id).ok()?;
    let expected_buf = hex_decode(&expected);
    let actual_buf = hex_decode(sig);
    if expected_buf.len() != actual_buf.len() {
        return None;
    }
    if !timing_safe_equal(&expected_buf, &actual_buf) {
        return None;
    }
    Some(token_id.to_string())
}

pub(crate) struct AuthState {
    secret: String,
    save_token: Arc<dyn DynSaveToken>,
    find_token: Arc<dyn DynFindToken>,
    revoke_token: Option<Arc<dyn DynRevokeToken>>,
    token_expiry: u64,
    hmac_algorithm: String,
    token_id_size: usize,
    pbkdf_opts: PbkdfOptions,
}

pub fn auth(options: AuthOptions) -> impl Middleware {
    if options.secret.chars().count() < 32 {
        throw(framework_error(
            "Secret must be at least 32 characters. HMAC security is only as strong as the key.",
            ErrorCode::WeakSecret,
            None,
            false,
        ));
    }
    if options.save_token.missing || options.find_token.missing {
        panic!("auth: the save_token and find_token options are required");
    }
    // Catch a typo in the algorithm names right away, instead of on the first login
    if let Err(error) = hmac_hex(&options.hmac_algorithm, &options.secret, "") {
        panic!("auth: {error}");
    }
    if let Err(error) = pbkdf2_sync(b"", b"", 1, 1, &options.digest) {
        panic!("auth: {error}");
    }

    let AuthOptions {
        secret,
        save_token,
        find_token,
        revoke_token,
        token_expiry,
        hmac_algorithm,
        token_id_size,
        ..
    } = options;

    let pbkdf_opts = PbkdfOptions {
        iterations: options.iterations,
        keylen: options.keylen,
        digest: options.digest,
        salt_size: options.salt_size,
    };

    // The state stays around for as long as the program runs (like a static), so every request
    // can use it without touching a counter that all the worker threads share
    let state: &'static AuthState = Box::leak(Box::new(AuthState {
        secret,
        save_token: save_token.callback,
        find_token: find_token.callback,
        revoke_token: revoke_token.map(|revoke_token| revoke_token.callback),
        token_expiry,
        hmac_algorithm,
        token_id_size,
        pbkdf_opts,
    }));

    async move |req: &mut CpeakRequest, _res: &mut CpeakResponse, next: Next| {
        req.auth = Some(state);
        next()
    }
}

// The helper methods that the auth() middleware gives you on req
impl CpeakRequest {
    // Hashes a password with PBKDF2. Store the result; never store plaintext.
    pub async fn hash_password(&self, password: &str) -> Result<String, CpeakError> {
        let state = self.auth_state("hash_password")?;
        hash_password(password, state.pbkdf_opts.clone()).await
    }

    // Verifies the password and if correct, creates a signed token.
    // Returns the token string to send to the client, or None on wrong password.
    pub async fn login(
        &self,
        password: &str,
        hashed_password: &str,
        user_id: &str,
    ) -> Result<Option<String>, CpeakError> {
        let state = self.auth_state("login")?;
        let is_match = verify_password(password, hashed_password).await?;
        if !is_match {
            return Ok(None);
        }
        let token_id = hex_encode(&random_bytes(state.token_id_size)?);
        let token = sign_token(&token_id, &state.secret, &state.hmac_algorithm)?;
        let expires_at = SystemTime::now() + Duration::from_millis(state.token_expiry);
        state
            .save_token
            .call(token_id, user_id.to_string(), expires_at)
            .await?;
        Ok(Some(token))
    }

    // Validates a token's HMAC signature and expiry. Returns the user_id, or None.
    pub async fn verify_token(&self, token: &str) -> Result<Option<VerifiedToken>, CpeakError> {
        let state = self.auth_state("verify_token")?;
        if token.is_empty() {
            return Ok(None);
        }
        let Some(token_id) = extract_token_id(token, &state.secret, &state.hmac_algorithm) else {
            return Ok(None);
        };
        let Some(record) = state.find_token.call(token_id).await? else {
            return Ok(None);
        };
        if record.expires_at < SystemTime::now() {
            return Ok(None);
        }
        Ok(Some(VerifiedToken {
            user_id: record.user_id,
        }))
    }

    // Revokes the token via your revoke_token callback. Only available when revoke_token is provided.
    pub async fn logout(&self, token: &str) -> Result<bool, CpeakError> {
        let state = self.auth_state("logout")?;
        let Some(revoke_token) = &state.revoke_token else {
            return Err(framework_error(
                "req.logout is only available when you pass the revoke_token option to auth().",
                ErrorCode::AuthNotEnabled,
                None,
                false,
            ));
        };
        let Some(token_id) = extract_token_id(token, &state.secret, &state.hmac_algorithm) else {
            return Ok(false);
        };
        revoke_token.call(token_id).await?;
        Ok(true)
    }

    fn auth_state(&self, method: &str) -> Result<&AuthState, CpeakError> {
        self.auth.ok_or_else(|| {
            framework_error(
                format!("auth middleware not registered. Add auth() via app.before_each(auth(...)) to use req.{method}."),
                ErrorCode::AuthNotEnabled,
                None,
                false,
            )
        })
    }
}

// PBKDF2 is slow on purpose, so we run it on a separate thread (like Node.js does with its
// thread pool) instead of blocking the event loop that's serving the other requests.
async fn pbkdf2_async(
    password: &str,
    salt: Vec<u8>,
    iterations: u32,
    keylen: usize,
    digest: &str,
) -> Result<Vec<u8>, CpeakError> {
    let password = password.as_bytes().to_vec();
    let digest = digest.to_string();
    tokio::task::spawn_blocking(move || pbkdf2_sync(&password, &salt, iterations, keylen, &digest))
        .await
        .map_err(|error| CpeakError::other(format!("PBKDF2 failed: {error}")))?
}

fn pbkdf2_sync(
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    keylen: usize,
    digest: &str,
) -> Result<Vec<u8>, CpeakError> {
    if iterations == 0 {
        return Err(CpeakError::other("The PBKDF2 iterations must be >= 1"));
    }
    let mut key = vec![0u8; keylen];
    match digest.to_lowercase().as_str() {
        "sha256" => pbkdf2::pbkdf2_hmac::<Sha256>(password, salt, iterations, &mut key),
        "sha384" => pbkdf2::pbkdf2_hmac::<Sha384>(password, salt, iterations, &mut key),
        "sha512" => pbkdf2::pbkdf2_hmac::<Sha512>(password, salt, iterations, &mut key),
        _ => {
            return Err(CpeakError::other(format!(
                "Invalid digest: \"{digest}\". Use \"sha256\", \"sha384\" or \"sha512\"."
            )))
        }
    }
    Ok(key)
}

fn hmac_hex(algorithm: &str, secret: &str, data: &str) -> Result<String, CpeakError> {
    fn sign<M: Mac + hmac::digest::KeyInit>(secret: &str, data: &str) -> Vec<u8> {
        let mut mac = <M as hmac::digest::KeyInit>::new_from_slice(secret.as_bytes())
            .expect("HMAC accepts keys of any size");
        mac.update(data.as_bytes());
        mac.finalize().into_bytes().to_vec()
    }

    let sig = match algorithm {
        _ if algorithm.eq_ignore_ascii_case("sha256") => sign::<Hmac<Sha256>>(secret, data),
        _ if algorithm.eq_ignore_ascii_case("sha384") => sign::<Hmac<Sha384>>(secret, data),
        _ if algorithm.eq_ignore_ascii_case("sha512") => sign::<Hmac<Sha512>>(secret, data),
        _ => {
            return Err(CpeakError::other(format!(
                "Invalid HMAC algorithm: \"{algorithm}\". Use \"sha256\", \"sha384\" or \"sha512\"."
            )))
        }
    };
    Ok(hex_encode(&sig))
}

fn random_bytes(size: usize) -> Result<Vec<u8>, CpeakError> {
    let mut bytes = vec![0u8; size];
    getrandom::fill(&mut bytes)
        .map_err(|error| CpeakError::other(format!("Failed to generate random bytes: {error}")))?;
    Ok(bytes)
}

// A constant-time comparison, so attackers can't learn anything from how long it takes
fn timing_safe_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        hex.push(HEX_DIGITS[(byte >> 4) as usize] as char);
        hex.push(HEX_DIGITS[(byte & 0xf) as usize] as char);
    }
    hex
}

// Like Node.js's Buffer.from(hex, "hex"): decodes the pairs of hex digits up to the first invalid one
fn hex_decode(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks_exact(2)
        .map_while(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

// Like JavaScript's parseInt(value, 10): reads the leading digits and ignores the rest
fn parse_int(value: &str) -> Option<i64> {
    let value = value.trim_start();
    let (sign, digits) = match value.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, value.strip_prefix('+').unwrap_or(value)),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    digits[..end]
        .parse::<i64>()
        .ok()
        .map(|number| sign * number)
}
