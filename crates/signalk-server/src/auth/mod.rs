use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use axum_extra::{
    headers::authorization::Bearer,
    TypedHeader,
};
use chrono::Utc;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use signalk_types::{LoginRequest, LoginResponse};
use std::sync::Arc;

use crate::ServerState;

/// JWT claims payload
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String, // username
    pub exp: usize,  // expiry timestamp (unix epoch)
    pub iat: usize,  // issued-at timestamp
}

/// Validate a JWT token string and return the claims.
pub fn validate_token(token: &str, secret: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
    let key = DecodingKey::from_secret(secret.as_bytes());
    let data = decode::<Claims>(token, &key, &Validation::default())?;
    Ok(data.claims)
}

/// Create a signed JWT token for a user.
pub fn create_token(
    username: &str,
    secret: &str,
    ttl_secs: u64,
) -> Result<String, jsonwebtoken::errors::Error> {
    let now = Utc::now().timestamp() as usize;
    let claims = Claims {
        sub: username.to_string(),
        iat: now,
        exp: now + ttl_secs as usize,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
}

// ─── Axum handlers ────────────────────────────────────────────────────────────

pub async fn login(
    State(state): State<Arc<ServerState>>,
    Json(req): Json<LoginRequest>,
) -> Response {
    if req.username != state.config.auth.admin_user {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"message": "Invalid credentials"})),
        )
            .into_response();
    }

    // Dev mode: accept any password when hash is empty
    // TODO: bcrypt/argon2 comparison for production
    if !state.config.auth.admin_password_hash.is_empty() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"message": "Invalid credentials"})),
        )
            .into_response();
    }

    match create_token(
        &req.username,
        &state.config.auth.jwt_secret,
        state.config.auth.token_ttl_secs,
    ) {
        Ok(token) => (
            StatusCode::OK,
            Json(LoginResponse {
                token,
                time_to_live: Some(state.config.auth.token_ttl_secs),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"message": e.to_string()})),
        )
            .into_response(),
    }
}

pub async fn validate(
    State(state): State<Arc<ServerState>>,
    TypedHeader(auth): TypedHeader<axum_extra::headers::Authorization<Bearer>>,
) -> Response {
    match validate_token(auth.token(), &state.config.auth.jwt_secret) {
        Ok(claims) => match create_token(
            &claims.sub,
            &state.config.auth.jwt_secret,
            state.config.auth.token_ttl_secs,
        ) {
            Ok(token) => (
                StatusCode::OK,
                Json(LoginResponse {
                    token,
                    time_to_live: Some(state.config.auth.token_ttl_secs),
                }),
            )
                .into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"message": e.to_string()})),
            )
                .into_response(),
        },
        Err(_) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"message": "Invalid or expired token"})),
        )
            .into_response(),
    }
}

pub async fn logout() -> Response {
    // Stateless JWT — logout handled client-side
    // TODO: token blocklist for production
    (StatusCode::OK, Json(serde_json::json!({"message": "Logged out"}))).into_response()
}
