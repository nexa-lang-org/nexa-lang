use anyhow::{anyhow, Result};
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

use crate::application::ports::storage::{TokenStore, UserStore};
use crate::domain::token::ApiToken;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String, // user_id as string
    exp: i64,    // unix timestamp
}

pub struct AuthService {
    user_store: Arc<dyn UserStore>,
    token_store: Arc<dyn TokenStore>,
    jwt_secret: String,
}

impl AuthService {
    pub fn new(
        user_store: Arc<dyn UserStore>,
        token_store: Arc<dyn TokenStore>,
        jwt_secret: String,
    ) -> Self {
        Self {
            user_store,
            token_store,
            jwt_secret,
        }
    }

    pub async fn register(&self, email: &str, password: &str) -> Result<String> {
        if self.user_store.find_by_email(email).await?.is_some() {
            return Err(anyhow!("email already registered"));
        }
        let hash =
            bcrypt::hash(password, bcrypt::DEFAULT_COST).map_err(|e| anyhow!("hash error: {e}"))?;
        let user = self.user_store.create(email, &hash).await?;
        self.make_jwt(user.id)
    }

    pub async fn login(&self, email: &str, password: &str) -> Result<String> {
        let user = self
            .user_store
            .find_by_email(email)
            .await?
            .ok_or_else(|| anyhow!("invalid email or password"))?;

        let valid = bcrypt::verify(password, &user.password_hash)
            .map_err(|e| anyhow!("verify error: {e}"))?;
        if !valid {
            return Err(anyhow!("invalid email or password"));
        }
        self.make_jwt(user.id)
    }

    /// Verify either a JWT session token or a permanent API token (`nxt_…`).
    /// Returns the authenticated user's UUID.
    pub async fn verify_token(&self, token: &str) -> Result<Uuid> {
        if token.starts_with("nxt_") {
            return self.verify_api_token(token).await;
        }
        self.verify_jwt(token)
    }

    // ── API token management ─────────────────────────────────────────────────

    /// Generate a new permanent API token for `user_id`.
    /// Returns `(raw_token, ApiToken)` — the raw value is shown only once.
    pub async fn create_api_token(
        &self,
        user_id: Uuid,
        name: &str,
    ) -> Result<(String, ApiToken)> {
        let raw = generate_token();
        let hash = hash_token(&raw);
        let record = self.token_store.create(user_id, name, &hash).await?;
        Ok((raw, record))
    }

    pub async fn list_api_tokens(&self, user_id: Uuid) -> Result<Vec<ApiToken>> {
        self.token_store.list_for_user(user_id).await
    }

    pub async fn revoke_api_token(&self, token_id: Uuid, user_id: Uuid) -> Result<bool> {
        self.token_store.delete(token_id, user_id).await
    }

    // ── Private helpers ──────────────────────────────────────────────────────

    fn verify_jwt(&self, token: &str) -> Result<Uuid> {
        let data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.jwt_secret.as_bytes()),
            &Validation::new(Algorithm::HS256),
        )
        .map_err(|e| anyhow!("invalid token: {e}"))?;
        Uuid::parse_str(&data.claims.sub).map_err(|e| anyhow!("invalid token subject: {e}"))
    }

    async fn verify_api_token(&self, raw: &str) -> Result<Uuid> {
        let hash = hash_token(raw);
        let record = self
            .token_store
            .find_by_hash(&hash)
            .await?
            .ok_or_else(|| anyhow!("invalid token"))?;
        Ok(record.user_id)
    }

    fn make_jwt(&self, user_id: Uuid) -> Result<String> {
        let exp = (Utc::now() + Duration::hours(24)).timestamp();
        let claims = Claims {
            sub: user_id.to_string(),
            exp,
        };
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.jwt_secret.as_bytes()),
        )
        .map_err(|e| anyhow!("encode error: {e}"))
    }
}

// ── Token generation / hashing ────────────────────────────────────────────────

/// Generate a random `nxt_<64 hex chars>` token (32 random bytes).
fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("nxt_{}", hex::encode(bytes))
}

/// SHA-256 hash of the raw token, hex-encoded.
fn hash_token(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}
