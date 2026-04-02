use anyhow::{anyhow, Result};
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::application::ports::storage::UserStore;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String, // user_id as string
    exp: i64,    // unix timestamp
}

pub struct AuthService {
    store: Arc<dyn UserStore>,
    jwt_secret: String,
}

impl AuthService {
    pub fn new(store: Arc<dyn UserStore>, jwt_secret: String) -> Self {
        Self { store, jwt_secret }
    }

    pub async fn register(&self, email: &str, password: &str) -> Result<String> {
        if self.store.find_by_email(email).await?.is_some() {
            return Err(anyhow!("email already registered"));
        }
        let hash =
            bcrypt::hash(password, bcrypt::DEFAULT_COST).map_err(|e| anyhow!("hash error: {e}"))?;
        let user = self.store.create(email, &hash).await?;
        self.make_token(user.id)
    }

    pub async fn login(&self, email: &str, password: &str) -> Result<String> {
        let user = self
            .store
            .find_by_email(email)
            .await?
            .ok_or_else(|| anyhow!("invalid email or password"))?;

        let valid = bcrypt::verify(password, &user.password_hash)
            .map_err(|e| anyhow!("verify error: {e}"))?;
        if !valid {
            return Err(anyhow!("invalid email or password"));
        }
        self.make_token(user.id)
    }

    pub fn verify_token(&self, token: &str) -> Result<Uuid> {
        let data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.jwt_secret.as_bytes()),
            &Validation::new(Algorithm::HS256),
        )
        .map_err(|e| anyhow!("invalid token: {e}"))?;
        let id =
            Uuid::parse_str(&data.claims.sub).map_err(|e| anyhow!("invalid token subject: {e}"))?;
        Ok(id)
    }

    fn make_token(&self, user_id: Uuid) -> Result<String> {
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
