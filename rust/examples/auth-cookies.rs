// Run it with: cargo run --example auth-cookies
#![allow(dead_code, unused_variables, clippy::needless_return)]

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use cpeak::{
    auth, cookie_parser, cpeak, parse_json, AuthOptions, CookieOptions, CpeakError, CpeakRequest,
    CpeakResponse, FindToken, Next, RevokeToken, SameSite, SaveToken, TokenRecord,
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

pub async fn require_auth(
    req: &mut CpeakRequest,
    res: &mut CpeakResponse,
    next: Next,
) -> Result<(), CpeakError> {
    let Some(token) = req.cookies.get("session") else {
        return Err(CpeakError::new(401, "Unauthorized."));
    };

    let Some(result) = req.verify_token(token).await? else {
        return Err(CpeakError::new(401, "Unauthorized."));
    };

    req.set("user", json!({ "id": result.user_id }));
    next()
}

pub async fn optional_auth(
    req: &mut CpeakRequest,
    _res: &mut CpeakResponse,
    next: Next,
) -> Result<(), CpeakError> {
    if let Some(token) = req.cookies.get("session") {
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
            let row = TOKENS.lock().unwrap().get(&token_id).cloned();
            Ok(row)
        }),
        revoke_token: Some(RevokeToken::new(async |token_id| {
            TOKENS.lock().unwrap().remove(&token_id);
            Ok(())
        })),
        ..Default::default()
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

        let token = req
            .login(&password, &hash, &user.id)
            .await?
            .unwrap_or_default();

        // The token is stored in an httpOnly cookie so it's never accessible via JavaScript, which protects against XSS attacks.
        res.cookie(
            "session",
            &token,
            CookieOptions {
                http_only: true,
                secure: true,
                same_site: Some(SameSite::Lax),
                ..Default::default()
            },
        );
        return res.status(201).json(json!({ "ok": true }));
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

        let token = req.login(&password, &user.password, &user.id).await?;

        let Some(token) = token else {
            return Err(CpeakError::new(401, "Invalid username or password"));
        };

        // The token is stored in an httpOnly cookie so it's never accessible via JavaScript, which protects against XSS attacks.
        res.cookie(
            "session",
            &token,
            CookieOptions {
                http_only: true,
                secure: true,
                same_site: Some(SameSite::Lax),
                ..Default::default()
            },
        );
        return res.json(json!({ "ok": true }));
    });

    app.route_with("get", "/profile", require_auth, async |req, res| {
        return res.json(json!({ "user": req.get::<Value>("user") }));
    });

    app.route_with("delete", "/logout", require_auth, async |req, res| {
        if let Some(token) = req.cookies.get("session") {
            req.logout(token).await?;
        }

        // It is your responsibility to clear the token from the client side as well.
        res.clear_cookie("session", None);
        return res.status(200).json(json!({ "message": "logged out" }));
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

    app.listen(9001, || {
        println!("Server has started on port 9001");
    })
    .wait();
}
