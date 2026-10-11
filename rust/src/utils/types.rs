use std::fmt;
use std::sync::Arc;
use std::time::SystemTime;

use regex::Regex;
use serde::Serialize;

use crate::internal::errors::CpeakError;
use crate::types::LocalBoxFuture;
use crate::utils::auth::DEFAULTS;

#[derive(Debug, Clone)]
pub struct PbkdfOptions {
    pub iterations: u32,
    pub keylen: usize,
    pub digest: String,
    pub salt_size: usize,
}

impl Default for PbkdfOptions {
    fn default() -> Self {
        Self {
            iterations: DEFAULTS.iterations,
            keylen: DEFAULTS.keylen,
            digest: DEFAULTS.digest.to_string(),
            salt_size: DEFAULTS.salt_size,
        }
    }
}

pub struct AuthOptions {
    pub secret: String,
    pub save_token: SaveToken,
    pub find_token: FindToken,
    // Token lifetime in milliseconds
    pub token_expiry: u64,
    pub hmac_algorithm: String,
    pub token_id_size: usize,
    pub revoke_token: Option<RevokeToken>,
    // The PBKDF2 options (the same as in PbkdfOptions)
    pub iterations: u32,
    pub keylen: usize,
    pub digest: String,
    pub salt_size: usize,
}

impl Default for AuthOptions {
    fn default() -> Self {
        Self {
            secret: String::new(),
            save_token: SaveToken::missing(),
            find_token: FindToken::missing(),
            token_expiry: DEFAULTS.token_expiry,
            hmac_algorithm: DEFAULTS.hmac_algorithm.to_string(),
            token_id_size: DEFAULTS.token_id_size,
            revoke_token: None,
            iterations: DEFAULTS.iterations,
            keylen: DEFAULTS.keylen,
            digest: DEFAULTS.digest.to_string(),
            salt_size: DEFAULTS.salt_size,
        }
    }
}

// What your findToken gives back: the user that the token belongs to, and when it expires
#[derive(Debug, Clone)]
pub struct TokenRecord {
    pub user_id: String,
    pub expires_at: SystemTime,
}

// What req.verify_token() gives back when the token is valid
#[derive(Debug, Clone, Serialize)]
pub struct VerifiedToken {
    pub user_id: String,
}

// saveToken: async (tokenId, userId, expiresAt) => { /* store in your DB */ }
pub struct SaveToken {
    pub(crate) callback: Arc<dyn DynSaveToken>,
    pub(crate) missing: bool,
}

impl SaveToken {
    pub fn new<F>(save_token: F) -> Self
    where
        F: AsyncFn(String, String, SystemTime) -> Result<(), CpeakError> + Send + Sync + 'static,
    {
        Self {
            callback: Arc::new(save_token),
            missing: false,
        }
    }

    fn missing() -> Self {
        let mut save_token = Self::new(async |_token_id, _user_id, _expires_at| Ok(()));
        save_token.missing = true;
        save_token
    }
}

// findToken: async (tokenId) => { /* return { userId, expiresAt } or null */ }
pub struct FindToken {
    pub(crate) callback: Arc<dyn DynFindToken>,
    pub(crate) missing: bool,
}

impl FindToken {
    pub fn new<F>(find_token: F) -> Self
    where
        F: AsyncFn(String) -> Result<Option<TokenRecord>, CpeakError> + Send + Sync + 'static,
    {
        Self {
            callback: Arc::new(find_token),
            missing: false,
        }
    }

    fn missing() -> Self {
        let mut find_token = Self::new(async |_token_id| Ok(None));
        find_token.missing = true;
        find_token
    }
}

// revokeToken: async (tokenId) => { /* delete from your DB */ }
pub struct RevokeToken {
    pub(crate) callback: Arc<dyn DynRevokeToken>,
}

impl RevokeToken {
    pub fn new<F>(revoke_token: F) -> Self
    where
        F: AsyncFn(String) -> Result<(), CpeakError> + Send + Sync + 'static,
    {
        Self {
            callback: Arc::new(revoke_token),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CookieOptions {
    pub signed: bool,
    pub http_only: bool,
    pub secure: bool,
    pub same_site: Option<SameSite>,
    pub max_age: Option<i64>, // ms
    pub expires: Option<SystemTime>,
    pub path: Option<String>,
    pub domain: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

impl SameSite {
    pub fn as_str(&self) -> &'static str {
        match self {
            SameSite::Strict => "Strict",
            SameSite::Lax => "Lax",
            SameSite::None => "None",
        }
    }
}

pub enum OriginInput {
    String(String),
    List(Vec<String>),
    RegExp(Regex),
    Bool(bool),
    Function(OriginFunction),
}

impl OriginInput {
    // A custom function to determine if the origin is allowed:
    // OriginInput::function(async |origin| origin == "https://example.com")
    pub fn function<F>(is_allowed: F) -> Self
    where
        F: AsyncFn(&str) -> bool + Send + Sync + 'static,
    {
        OriginInput::Function(OriginFunction(Box::new(is_allowed)))
    }
}

impl From<&str> for OriginInput {
    fn from(origin: &str) -> Self {
        OriginInput::String(origin.to_string())
    }
}

impl From<String> for OriginInput {
    fn from(origin: String) -> Self {
        OriginInput::String(origin)
    }
}

impl From<Vec<&str>> for OriginInput {
    fn from(origins: Vec<&str>) -> Self {
        OriginInput::List(origins.into_iter().map(str::to_string).collect())
    }
}

impl From<Vec<String>> for OriginInput {
    fn from(origins: Vec<String>) -> Self {
        OriginInput::List(origins)
    }
}

impl<const N: usize> From<[&str; N]> for OriginInput {
    fn from(origins: [&str; N]) -> Self {
        OriginInput::List(origins.into_iter().map(str::to_string).collect())
    }
}

impl From<Regex> for OriginInput {
    fn from(origin: Regex) -> Self {
        OriginInput::RegExp(origin)
    }
}

impl From<bool> for OriginInput {
    fn from(origin: bool) -> Self {
        OriginInput::Bool(origin)
    }
}

impl fmt::Debug for OriginInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OriginInput::String(origin) => f.debug_tuple("String").field(origin).finish(),
            OriginInput::List(origins) => f.debug_tuple("List").field(origins).finish(),
            OriginInput::RegExp(origin) => f.debug_tuple("RegExp").field(origin).finish(),
            OriginInput::Bool(origin) => f.debug_tuple("Bool").field(origin).finish(),
            OriginInput::Function(_) => f.write_str("Function"),
        }
    }
}

pub struct OriginFunction(pub(crate) Box<dyn DynOriginFunction>);

#[derive(Debug)]
pub struct CorsOptions {
    pub origin: OriginInput,
    pub methods: String,
    // An empty string means it's not set
    pub allowed_headers: String,
    // An empty string means it's not set
    pub exposed_headers: String,
    pub credentials: bool,
    pub max_age: u64,
    pub preflight_continue: bool,
    pub options_success_status: u16,
}

impl Default for CorsOptions {
    fn default() -> Self {
        Self {
            origin: OriginInput::String("*".to_string()),
            methods: "GET,HEAD,PUT,PATCH,POST,DELETE".to_string(),
            allowed_headers: String::new(),
            exposed_headers: String::new(),
            credentials: false,
            max_age: 86400,
            preflight_continue: false,
            options_success_status: 204,
        }
    }
}

// ---------------------------------------------------------------------------------------
// Internals: object-safe wrappers around the AsyncFn callbacks above.

pub(crate) trait DynSaveToken: Send + Sync {
    fn call(
        &self,
        token_id: String,
        user_id: String,
        expires_at: SystemTime,
    ) -> LocalBoxFuture<'_, Result<(), CpeakError>>;
}

impl<F> DynSaveToken for F
where
    F: AsyncFn(String, String, SystemTime) -> Result<(), CpeakError> + Send + Sync,
{
    fn call(
        &self,
        token_id: String,
        user_id: String,
        expires_at: SystemTime,
    ) -> LocalBoxFuture<'_, Result<(), CpeakError>> {
        Box::pin(self(token_id, user_id, expires_at))
    }
}

pub(crate) trait DynFindToken: Send + Sync {
    fn call(&self, token_id: String)
        -> LocalBoxFuture<'_, Result<Option<TokenRecord>, CpeakError>>;
}

impl<F> DynFindToken for F
where
    F: AsyncFn(String) -> Result<Option<TokenRecord>, CpeakError> + Send + Sync,
{
    fn call(
        &self,
        token_id: String,
    ) -> LocalBoxFuture<'_, Result<Option<TokenRecord>, CpeakError>> {
        Box::pin(self(token_id))
    }
}

pub(crate) trait DynRevokeToken: Send + Sync {
    fn call(&self, token_id: String) -> LocalBoxFuture<'_, Result<(), CpeakError>>;
}

impl<F> DynRevokeToken for F
where
    F: AsyncFn(String) -> Result<(), CpeakError> + Send + Sync,
{
    fn call(&self, token_id: String) -> LocalBoxFuture<'_, Result<(), CpeakError>> {
        Box::pin(self(token_id))
    }
}

pub(crate) trait DynOriginFunction: Send + Sync {
    fn call<'a>(&'a self, origin: &'a str) -> LocalBoxFuture<'a, bool>;
}

impl<F> DynOriginFunction for F
where
    F: AsyncFn(&str) -> bool + Send + Sync,
{
    fn call<'a>(&'a self, origin: &'a str) -> LocalBoxFuture<'a, bool> {
        Box::pin(self(origin))
    }
}
