// Authentication with the auth utility
mod common;

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime};

use common::{request, serve};
use cpeak::{
    auth, cpeak, hash_password, parse_json, verify_password, AuthOptions, CpeakError, CpeakRequest,
    CpeakResponse, FindToken, Next, PbkdfOptions, RevokeToken, SaveToken, TokenRecord,
};
use serde_json::{json, Value};

static TOKENS: LazyLock<Mutex<HashMap<String, TokenRecord>>> = LazyLock::new(Default::default);
static USERS: LazyLock<Mutex<Vec<User>>> = LazyLock::new(Default::default);

#[derive(Clone)]
struct User {
    id: String,
    username: String,
    password: String,
}

async fn require_auth(
    req: &mut CpeakRequest,
    _res: &mut CpeakResponse,
    next: Next,
) -> Result<(), CpeakError> {
    let Some(token) = req.headers.get("authorization") else {
        return Err(CpeakError::new(401, "Unauthorized."));
    };
    let Some(result) = req.verify_token(token).await? else {
        return Err(CpeakError::new(401, "Unauthorized."));
    };
    req.set("user", json!({ "id": result.user_id }));
    next()
}

fn server() -> u16 {
    serve(|| {
        let mut server = cpeak();
        server.before_each(parse_json(None));

        server.before_each(auth(AuthOptions {
            secret: "Soeofjeowjgogru9876%TRFYGHVJBH##".into(),
            save_token: SaveToken::new(async |token_id, user_id, expires_at| {
                TOKENS.lock().unwrap().insert(
                    token_id,
                    TokenRecord {
                        user_id,
                        expires_at,
                    },
                );
                Ok(())
            }),
            find_token: FindToken::new(async |token_id| {
                Ok(TOKENS.lock().unwrap().get(&token_id).cloned())
            }),
            revoke_token: Some(RevokeToken::new(async |token_id| {
                TOKENS.lock().unwrap().remove(&token_id);
                Ok(())
            })),
            iterations: 250_000,
            keylen: 128,
            digest: "sha256".into(),
            salt_size: 64,
            hmac_algorithm: "sha256".into(),
            token_id_size: 40,
            token_expiry: 3 * 24 * 60 * 60 * 1000,
        }));

        server.route("post", "/register", async |req, res| {
            let username = req.body["username"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let password = req.body["password"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let hash = req.hash_password(&password).await?;
            let user = {
                let mut users = USERS.lock().unwrap();
                let user = User {
                    id: (users.len() + 1).to_string(),
                    username,
                    password: hash.clone(),
                };
                users.push(user.clone());
                user
            };
            let token = req.login(&password, &hash, &user.id).await?;
            res.status(201).json(json!({ "token": token }))
        });

        server.route("post", "/login", async |req, res| {
            let username = req.body["username"].as_str().unwrap_or_default();
            let password = req.body["password"].as_str().unwrap_or_default();
            let user = USERS
                .lock()
                .unwrap()
                .iter()
                .find(|u| u.username == username)
                .cloned();
            let Some(user) = user else {
                return Err(CpeakError::new(401, "Invalid username or password"));
            };
            let Some(token) = req.login(password, &user.password, &user.id).await? else {
                return Err(CpeakError::new(401, "Invalid username or password"));
            };
            res.json(json!({ "token": token }))
        });

        server.route_with("get", "/profile", require_auth, async |req, res| {
            res.json(json!({ "user": req.get::<Value>("user") }))
        });

        server.route_with("delete", "/logout", require_auth, async |req, res| {
            if let Some(token) = req.headers.get("authorization") {
                req.logout(token).await?;
            }
            res.status(200).json(json!({ "message": "logged out" }))
        });

        server.handle_err(async |err, _req, res| {
            res.status(err.status.unwrap_or(500))
                .json(json!({ "error": err.message }))
        });

        server
    })
}

fn register(username: &str, password: &str) -> String {
    let res = request(server(), "POST", "/register")
        .json(&json!({ "username": username, "password": password }))
        .send();
    assert_eq!(res.status, 201);
    res.json()["token"].as_str().unwrap().to_string()
}

fn profile(token: Option<&str>) -> common::TestResponse {
    let req = request(server(), "GET", "/profile");
    match token {
        Some(token) => req.header("authorization", token).send(),
        None => req.send(),
    }
}

fn token_id_of(user_id: &str) -> Option<String> {
    TOKENS
        .lock()
        .unwrap()
        .iter()
        .find(|(_, record)| record.user_id == user_id)
        .map(|(token_id, _)| token_id.clone())
}

// ─── hash_password() ──────────────────────────────────────────────────────────

#[tokio::test]
async fn hash_password_returns_a_string_in_pbkdf2_iter_keylen_digest_salt_hex_hash_hex_format() {
    let hash = hash_password(
        "mypassword",
        PbkdfOptions {
            iterations: 1000,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let parts: Vec<&str> = hash.split(':').collect();
    assert_eq!(parts.len(), 6);
    assert_eq!(parts[0], "pbkdf2");
}

#[tokio::test]
async fn hash_password_embeds_the_correct_custom_options_in_the_hash_string() {
    let hash = hash_password(
        "mypassword",
        PbkdfOptions {
            iterations: 1000,
            keylen: 32,
            digest: "sha256".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let parts: Vec<&str> = hash.split(':').collect();
    assert_eq!(parts[1], "1000");
    assert_eq!(parts[2], "32");
    assert_eq!(parts[3], "sha256");
}

#[tokio::test]
async fn hash_password_produces_different_hashes_for_the_same_password_due_to_random_salt() {
    let options = PbkdfOptions {
        iterations: 1000,
        ..Default::default()
    };
    let hash1 = hash_password("mypassword", options.clone()).await.unwrap();
    let hash2 = hash_password("mypassword", options).await.unwrap();
    assert_ne!(hash1, hash2);
}

#[tokio::test]
async fn hash_password_resulting_hash_verifies_correctly_with_verify_password() {
    let hash = hash_password(
        "mypassword",
        PbkdfOptions {
            iterations: 1000,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(verify_password("mypassword", &hash).await.unwrap());
}

// ─── verify_password() ────────────────────────────────────────────────────────

#[tokio::test]
async fn verify_password_returns_true_for_the_correct_password() {
    let hash = hash_password(
        "correctpassword",
        PbkdfOptions {
            iterations: 1000,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(verify_password("correctpassword", &hash).await.unwrap());
}

#[tokio::test]
async fn verify_password_returns_false_for_the_wrong_password() {
    let hash = hash_password(
        "correctpassword",
        PbkdfOptions {
            iterations: 1000,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!verify_password("wrongpassword", &hash).await.unwrap());
}

#[tokio::test]
async fn verify_password_accepts_hashes_made_by_the_node_js_version() {
    // Made with `await hashPassword("s3cret!", { iterations: 1000 })` in the Node.js version
    let sha512 = "pbkdf2:1000:64:sha512:9e77c58ed8cda838025f050699a2f698befd2f985069297ad3a550423f8d74d6:d9b3a7064b265a63597f2c40556f41501ab8a3f02c5ac5e8724d55a829206dc40aa900408f35548d6f8241f4f9fbfb048a6439a6cd20744bc102e1f6c06e1674";
    // Made with `await hashPassword("s3cret!", { iterations: 1000, digest: "sha256", keylen: 32 })`
    let sha256 = "pbkdf2:1000:32:sha256:c9eca0c30ef531e23e55f07daee731fe90e91b9d60eb0ac56e0d9d123630001c:93bb35d7571c534c59ac9b68ee2f8be939d43060531311604f4e33cba8292f2e";

    assert!(verify_password("s3cret!", sha512).await.unwrap());
    assert!(verify_password("s3cret!", sha256).await.unwrap());
    assert!(!verify_password("wrong", sha512).await.unwrap());
}

#[tokio::test]
async fn verify_password_returns_false_for_malformed_hashes() {
    assert!(!verify_password("x", "not-a-hash").await.unwrap());
    assert!(!verify_password("x", "pbkdf2:abc:64:sha512:aa:bb")
        .await
        .unwrap());
    assert!(!verify_password("x", "pbkdf2:1000:64:sha512::bb")
        .await
        .unwrap());
}

// ─── auth() initialization ────────────────────────────────────────────────────

fn minimal_options(secret: String) -> AuthOptions {
    AuthOptions {
        secret,
        save_token: SaveToken::new(async |_token_id, _user_id, _expires_at| Ok(())),
        find_token: FindToken::new(async |_token_id| Ok(None)),
        ..Default::default()
    }
}

#[test]
#[should_panic(expected = "CPEAK_ERR_WEAK_SECRET")]
fn auth_throws_weak_secret_when_secret_is_missing() {
    let _ = auth(minimal_options(String::new()));
}

#[test]
#[should_panic(expected = "CPEAK_ERR_WEAK_SECRET")]
fn auth_throws_weak_secret_when_secret_is_31_characters() {
    let _ = auth(minimal_options("a".repeat(31)));
}

#[test]
fn auth_does_not_throw_when_secret_is_exactly_32_characters() {
    let _ = auth(minimal_options("a".repeat(32)));
}

// ─── req.logout presence ──────────────────────────────────────────────────────

#[test]
fn req_logout_is_not_available_when_revoke_token_is_not_provided() {
    let port = serve(|| {
        let mut server = cpeak();
        server.before_each(auth(minimal_options("a".repeat(32))));
        server.route("get", "/logout", async |req, res| {
            let logged_out = req.logout("anything").await?;
            res.json(json!({ "loggedOut": logged_out }))
        });
        server.handle_err(async |error, _req, res| {
            res.status(500).json(json!({ "code": error.code }))
        });
        server
    });

    let res = request(port, "GET", "/logout").send();
    assert_eq!(res.json()["code"], "CPEAK_ERR_AUTH_NOT_ENABLED");
}

// ─── POST /register ───────────────────────────────────────────────────────────

#[test]
fn register_returns_201_with_a_signed_token_in_token_id_dot_signature_format() {
    let token = register("alice", "password123");
    assert!(!token.is_empty());
    assert_eq!(token.split('.').count(), 2);
}

#[test]
fn register_produces_unique_tokens_for_different_registrations() {
    let token1 = register("bob", "bobpass");
    let token2 = register("carol", "carolpass");
    assert_ne!(token1, token2);
}

// ─── POST /login ──────────────────────────────────────────────────────────────

#[test]
fn login_returns_200_with_a_signed_token_for_correct_credentials() {
    register("dave", "davepass");
    let res = request(server(), "POST", "/login")
        .json(&json!({ "username": "dave", "password": "davepass" }))
        .send();
    assert_eq!(res.status, 200);
    assert_eq!(res.json()["token"].as_str().unwrap().split('.').count(), 2);
}

#[test]
fn login_returns_401_for_wrong_password() {
    register("dave2", "davepass");
    let res = request(server(), "POST", "/login")
        .json(&json!({ "username": "dave2", "password": "wrongpass" }))
        .send();
    assert_eq!(res.status, 401);
}

// ─── GET /profile (verify_token) ──────────────────────────────────────────────

#[test]
fn profile_allows_access_and_returns_the_correct_user_id_with_a_valid_token() {
    let valid_token = register("eve", "evepass");
    let valid_user_id = profile(Some(&valid_token)).json()["user"]["id"].clone();

    let res = profile(Some(&valid_token));
    assert_eq!(res.status, 200);
    assert_eq!(res.json()["user"]["id"], valid_user_id);
}

#[test]
fn profile_returns_401_when_no_authorization_header_is_present() {
    assert_eq!(profile(None).status, 401);
}

#[test]
fn profile_returns_401_for_a_token_without_a_dot_separator() {
    assert_eq!(profile(Some("badtokennodot")).status, 401);
}

#[test]
fn profile_returns_401_for_a_tampered_token_modified_signature() {
    let valid_token = register("eve2", "evepass");
    let tampered = format!("{}0000", &valid_token[..valid_token.len() - 4]);
    assert_eq!(profile(Some(&tampered)).status, 401);
}

#[test]
fn profile_returns_401_when_the_token_has_been_removed_from_the_store() {
    let token = register("frank", "frankpass");
    let frank_user_id = profile(Some(&token)).json()["user"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let frank_token_id = token_id_of(&frank_user_id);
    assert!(
        frank_token_id.is_some(),
        "frank's token should exist in the store before deletion"
    );
    TOKENS.lock().unwrap().remove(&frank_token_id.unwrap());

    assert_eq!(profile(Some(&token)).status, 401);
}

#[test]
fn profile_returns_401_when_the_token_has_expired() {
    let token = register("grace", "gracepass");
    let grace_user_id = profile(Some(&token)).json()["user"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let grace_token_id = token_id_of(&grace_user_id);
    assert!(
        grace_token_id.is_some(),
        "grace's token should exist in the store before expiry"
    );
    TOKENS
        .lock()
        .unwrap()
        .get_mut(&grace_token_id.unwrap())
        .unwrap()
        .expires_at = SystemTime::now() - Duration::from_secs(1);

    assert_eq!(profile(Some(&token)).status, 401);
}

// ─── DELETE /logout ───────────────────────────────────────────────────────────

#[test]
fn logout_returns_200_on_successful_logout_and_subsequent_requests_with_the_revoked_token_return_401(
) {
    let logout_token = register("henry", "henrypass");

    let res = request(server(), "DELETE", "/logout")
        .header("authorization", &logout_token)
        .send();
    assert_eq!(res.status, 200);
    assert_eq!(res.json(), json!({ "message": "logged out" }));

    assert_eq!(profile(Some(&logout_token)).status, 401);
}

#[test]
fn logout_returns_401_when_no_authorization_header_is_provided() {
    let res = request(server(), "DELETE", "/logout").send();
    assert_eq!(res.status, 401);
}

#[test]
fn logout_returns_401_when_a_tampered_token_is_provided() {
    let token = register("irene", "irenepass");
    let tampered = format!("{}0000", &token[..token.len() - 4]);
    let res = request(server(), "DELETE", "/logout")
        .header("authorization", &tampered)
        .send();
    assert_eq!(res.status, 401);
}
