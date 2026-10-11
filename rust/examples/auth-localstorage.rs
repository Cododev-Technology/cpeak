// Run it with: cargo run --example auth-localstorage
#![allow(dead_code, unused_variables, clippy::needless_return)]

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use cpeak::{
    auth, cookie_parser, cpeak, parse_json, AuthOptions, CpeakError, CpeakRequest, CpeakResponse,
    FindToken, Next, RevokeToken, SaveToken, TokenRecord,
};
use serde::Deserialize;
use serde_json::{json, Value};

// In-memory storage for tokens and users. In a real application, you would use a database for them.
static TOKENS: LazyLock<Mutex<HashMap<String, TokenRecord>>> = LazyLock::new(Default::default);
static USERS: LazyLock<Mutex<Vec<User>>> = LazyLock::new(Default::default);

#[derive(Clone)]
struct User {
    id: String,
    username: String,
    password: String,
}

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
}

// This middleware will check for the token and set the user if the token is valid, but it will throw an error
// if the token is missing or invalid.
pub async fn require_auth(
    req: &mut CpeakRequest,
    res: &mut CpeakResponse,
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

// This middleware will check for the token and set the user if the token is valid, but it won't throw an error if the token is missing or invalid.
// This can be useful for routes that can be accessed by both authenticated and unauthenticated users.
pub async fn optional_auth(
    req: &mut CpeakRequest,
    _res: &mut CpeakResponse,
    next: Next,
) -> Result<(), CpeakError> {
    if let Some(token) = req.headers.get("authorization") {
        if let Some(result) = req.verify_token(token).await? {
            req.set("user", json!({ "id": result.user_id }));
        }
    }
    next()
}

fn main() {
    let mut app = cpeak();

    app.before_each(parse_json(None));
    app.before_each(cookie_parser(None));

    app.before_each(auth(AuthOptions {
        secret: "Soeofjeowjgogru9876%TRFYGHVJBH##".into(),
        save_token: SaveToken::new(async |token_id, user_id, expires_at| {
            TOKENS
                .lock()
                .unwrap()
                .insert(token_id, TokenRecord { user_id, expires_at });
            Ok(())
        }),
        find_token: FindToken::new(async |token_id| {
            let row = TOKENS.lock().unwrap().get(&token_id).cloned();
            Ok(row)
        }),
        revoke_token: Some(RevokeToken::new(async |token_id| {
            // This revokes the specific token for the current session. This can be useful for logout from a specific device,
            // for example, user logs out from their phone but they want to stay logged in on their laptop.
            TOKENS.lock().unwrap().remove(&token_id);

            // If you want to revoke all tokens for a user upon logout, look at the logout route below for an example.
            Ok(())
        })),

        /* For finer control you can specify all these below, but all are optional and have sensible defaults: */
        // Password hashing (PBKDF2)
        iterations: 210_000,      // Number of PBKDF2 iterations — higher = slower brute-force
        keylen: 64,               // Derived key length in bytes
        digest: "sha512".into(),  // Hash algorithm used by PBKDF2
        salt_size: 32,            // Random salt length in bytes

        // Token signing
        hmac_algorithm: "sha256".into(),        // Algorithm used to sign/verify token IDs
        token_id_size: 20,                      // Random token ID length in bytes before signing
        token_expiry: 7 * 24 * 60 * 60 * 1000,  // Token lifetime in ms (default: 7 days)
    }));

    app.route("post", "/register", async |req, res| {
        let Credentials { username, password } = serde_json::from_value(req.body.clone())?;

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
        return res.status(201).json(json!({ "token": token }));
    });

    app.route("post", "/login", async |req, res| {
        let Credentials { username, password } = serde_json::from_value(req.body.clone())?;

        let user = USERS
            .lock()
            .unwrap()
            .iter()
            .find(|u| u.username == username)
            .cloned();
        let Some(user) = user else {
            return Err(CpeakError::new(401, "Invalid username or password"));
        };

        // The client will then save this token and send it in the Authorization header for subsequent requests to protected routes.
        // You can optionally set this as a cookie as well. See the auth-cookies.rs example.
        let token = req.login(&password, &user.password, &user.id).await?;
        return res.json(json!({ "token": token }));
    });

    app.route_with("get", "/profile", require_auth, async |req, res| {
        return res.json(json!({ "user": req.get::<Value>("user") }));
    });

    app.route_with("delete", "/logout", require_auth, async |req, res| {
        if let Some(token) = req.headers.get("authorization") {
            req.logout(token).await?;
        }
        return res.status(200).json(json!({ "message": "logged out" }));

        // If you want to revoke all tokens for a user, you can do it like this if using SQL:
        // DELETE FROM tokens WHERE user_id = $1;
    });

    // Global error handler
    app.handle_err(async |error, req, res| {
        if let Some(status) = error.status {
            return res.status(status).json(json!({ "error": error.message }));
        } else {
            eprintln!("{error:?}");
            return res.status(500).json(json!({
                "error": "Sorry, something unexpected happened from our side."
            }));
        }
    });

    app.listen(9000, || {
        println!("Server has started on port 9000");
    })
    .wait();
}
