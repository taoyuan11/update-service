use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{AppState, error::ApiError, models::User};

pub const SESSION_COOKIE: &str = "update_session";

pub fn password_hash(password: &str) -> Result<String, ApiError> {
    use argon2::{
        Argon2, PasswordHasher,
        password_hash::{SaltString, rand_core::OsRng},
    };
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|v| v.to_string())
        .map_err(ApiError::internal)
}

pub fn verify_password(hash: &str, password: &str) -> bool {
    use argon2::{Argon2, PasswordVerifier, password_hash::PasswordHash};
    PasswordHash::new(hash)
        .ok()
        .and_then(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .ok()
        })
        .is_some()
}

pub async fn create_session(state: &AppState, user_id: Uuid) -> Result<(String, String), ApiError> {
    let token = random_token();
    let csrf = random_token();
    let token_hash = hex_hash(&token);
    sqlx::query("DELETE FROM sessions WHERE expires_at < NOW()")
        .execute(&state.db)
        .await?;
    sqlx::query("INSERT INTO sessions (id,user_id,token_hash,csrf_token,expires_at) VALUES ($1,$2,$3,$4,$5)")
        .bind(Uuid::new_v4()).bind(user_id).bind(token_hash).bind(&csrf).bind(Utc::now() + Duration::days(7))
        .execute(&state.db).await?;
    Ok((token, csrf))
}

pub fn session_cookie(state: &AppState, token: &str) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, token.to_owned()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(state.config.cookie_secure)
        .build()
}

pub fn clear_session_cookie() -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, ""))
        .path("/")
        .http_only(true)
        .max_age(time::Duration::seconds(0))
        .build()
}

pub async fn current_user(state: &AppState, jar: &CookieJar) -> Result<(User, String), ApiError> {
    let token = jar
        .get(SESSION_COOKIE)
        .map(|c| c.value().to_owned())
        .ok_or_else(|| ApiError::Unauthorized("Login required".into()))?;
    let token_hash = hex_hash(&token);
    let row = sqlx::query_as::<_, AuthRow>(
        "SELECT u.id, u.username, u.password_hash, u.role, u.enabled, u.created_at, u.updated_at, s.csrf_token FROM users u JOIN sessions s ON s.user_id=u.id WHERE s.token_hash=$1 AND s.expires_at > NOW() AND u.enabled=true"
    ).bind(token_hash).fetch_optional(&state.db).await?;
    row.map(|v| {
        (
            User {
                id: v.id,
                username: v.username,
                password_hash: v.password_hash,
                role: v.role,
                enabled: v.enabled,
                created_at: v.created_at,
                updated_at: v.updated_at,
            },
            v.csrf_token,
        )
    })
    .ok_or_else(|| ApiError::Unauthorized("Session expired".into()))
}

#[derive(FromRow)]
struct AuthRow {
    id: Uuid,
    username: String,
    password_hash: String,
    role: String,
    enabled: bool,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    csrf_token: String,
}

pub fn require_csrf(headers: &axum::http::HeaderMap, csrf: &str) -> Result<(), ApiError> {
    let supplied = headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if supplied.is_empty() || !constant_time_eq(supplied.as_bytes(), csrf.as_bytes()) {
        return Err(ApiError::Forbidden("Invalid CSRF token".into()));
    }
    Ok(())
}

fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn hex_hash(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn password_round_trip() {
        let hash = password_hash("correct horse battery staple").unwrap();
        assert!(verify_password(&hash, "correct horse battery staple"));
        assert!(!verify_password(&hash, "wrong"));
    }
}
