use anyhow::{anyhow, Result};
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

use crate::application::ports::storage::{RefreshTokenStore, TokenStore, UserStore};
use crate::domain::token::ApiToken;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String, // user_id as string
    exp: i64,    // unix timestamp
}

/// Returned by `login` and `register`.
/// `access_token` is a short-lived JWT; `refresh_token` is an opaque `nxr_`
/// token valid for 30 days that can be used with `POST /v1/auth/refresh`.
#[derive(Debug)]
pub struct LoginResult {
    pub access_token: String,
    pub refresh_token: String,
}

pub struct AuthService {
    user_store: Arc<dyn UserStore>,
    token_store: Arc<dyn TokenStore>,
    refresh_token_store: Arc<dyn RefreshTokenStore>,
    jwt_secret: String,
}

impl AuthService {
    pub fn new(
        user_store: Arc<dyn UserStore>,
        token_store: Arc<dyn TokenStore>,
        refresh_token_store: Arc<dyn RefreshTokenStore>,
        jwt_secret: String,
    ) -> Self {
        Self {
            user_store,
            token_store,
            refresh_token_store,
            jwt_secret,
        }
    }

    pub async fn register(&self, email: &str, password: &str) -> Result<LoginResult> {
        // Normalize to lowercase so Alice@example.com and alice@example.com
        // are treated as the same account (prevents squatting attacks).
        let email = email.to_lowercase();
        let email = email.as_str();
        if !valid_email(email) {
            return Err(anyhow!("invalid email address"));
        }
        if self.user_store.find_by_email(email).await?.is_some() {
            return Err(anyhow!("email already registered"));
        }
        let hash = bcrypt::hash(password, bcrypt::DEFAULT_COST).map_err(|e| {
            tracing::error!("bcrypt hash failed: {e}");
            anyhow!("registration failed")
        })?;
        let user = self.user_store.create(email, &hash).await?;
        let access_token = self.make_jwt(user.id)?;
        let refresh_token = self.issue_refresh_token(user.id).await?;
        Ok(LoginResult { access_token, refresh_token })
    }

    pub async fn login(&self, email: &str, password: &str) -> Result<LoginResult> {
        let email = email.to_lowercase();
        let email = email.as_str();
        let user = self
            .user_store
            .find_by_email(email)
            .await?
            .ok_or_else(|| anyhow!("invalid email or password"))?;

        let valid = bcrypt::verify(password, &user.password_hash).map_err(|e| {
            tracing::error!("bcrypt verify failed: {e}");
            anyhow!("invalid email or password")
        })?;
        if !valid {
            return Err(anyhow!("invalid email or password"));
        }
        let access_token = self.make_jwt(user.id)?;
        let refresh_token = self.issue_refresh_token(user.id).await?;
        Ok(LoginResult { access_token, refresh_token })
    }

    /// Issue a new access token using a valid refresh token.
    /// The refresh token must not be expired; it is NOT rotated (re-use allowed
    /// until expiry or explicit logout).
    pub async fn refresh_access(&self, raw_refresh: &str) -> Result<String> {
        let hash = hash_token(raw_refresh);
        let rt = self
            .refresh_token_store
            .find_by_hash(&hash)
            .await?
            .ok_or_else(|| anyhow!("invalid or expired refresh token"))?;
        if rt.expires_at < Utc::now() {
            let _ = self.refresh_token_store.delete_by_hash(&hash).await;
            return Err(anyhow!("refresh token expired"));
        }
        self.make_jwt(rt.user_id)
    }

    /// Revoke a refresh token (logout).
    pub async fn logout(&self, raw_refresh: &str) -> Result<()> {
        let hash = hash_token(raw_refresh);
        self.refresh_token_store.delete_by_hash(&hash).await?;
        Ok(())
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
    pub async fn create_api_token(&self, user_id: Uuid, name: &str) -> Result<(String, ApiToken)> {
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
        let exp = (Utc::now() + Duration::days(7)).timestamp();
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

    /// Generate and persist a 30-day refresh token for `user_id`.
    async fn issue_refresh_token(&self, user_id: Uuid) -> Result<String> {
        let raw = generate_refresh_token();
        let hash = hash_token(&raw);
        let expires_at = Utc::now() + Duration::days(30);
        self.refresh_token_store.create(user_id, &hash, expires_at).await?;
        Ok(raw)
    }
}

// ── Email validation ──────────────────────────────────────────────────────────

/// Basic structural email check: non-empty local + `@` + non-empty domain with at least one `.`.
/// Max length 254 (RFC 5321). Does not perform DNS lookup.
fn valid_email(email: &str) -> bool {
    if email.len() > 254 {
        return false;
    }
    let mut parts = email.splitn(2, '@');
    let local = parts.next().unwrap_or("");
    let domain = match parts.next() {
        Some(d) => d,
        None => return false,
    };
    !local.is_empty() && !domain.is_empty() && domain.contains('.')
}

// ── Token generation / hashing ────────────────────────────────────────────────

/// Generate a random `nxt_<64 hex chars>` API token (32 random bytes).
fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("nxt_{}", hex::encode(bytes))
}

/// Generate a random `nxr_<64 hex chars>` refresh token (32 random bytes).
fn generate_refresh_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    format!("nxr_{}", hex::encode(bytes))
}

/// SHA-256 hash of the raw token, hex-encoded.
fn hash_token(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{refresh_token::RefreshToken, token::ApiToken, user::User};
    use anyhow::Result;
    use async_trait::async_trait;
    use chrono::DateTime;
    use std::sync::Mutex;

    // ── In-memory UserStore ──────────────────────────────────────────────────

    #[derive(Default)]
    struct MemUserStore {
        users: Mutex<Vec<User>>,
    }

    #[async_trait]
    impl UserStore for MemUserStore {
        async fn create(&self, email: &str, password_hash: &str) -> Result<User> {
            let mut users = self.users.lock().unwrap();
            if users.iter().any(|u| u.email == email) {
                return Err(anyhow::anyhow!("email already registered"));
            }
            let user = User {
                id: Uuid::new_v4(),
                email: email.to_string(),
                password_hash: password_hash.to_string(),
                created_at: Utc::now(),
                signing_key: None,
            };
            users.push(user.clone());
            Ok(user)
        }

        async fn find_by_email(&self, email: &str) -> Result<Option<User>> {
            Ok(self
                .users
                .lock()
                .unwrap()
                .iter()
                .find(|u| u.email == email)
                .cloned())
        }

        async fn find_by_id(&self, id: Uuid) -> Result<Option<User>> {
            Ok(self
                .users
                .lock()
                .unwrap()
                .iter()
                .find(|u| u.id == id)
                .cloned())
        }

        async fn set_signing_key(&self, id: Uuid, pubkey: &str) -> Result<()> {
            let mut users = self.users.lock().unwrap();
            if let Some(u) = users.iter_mut().find(|u| u.id == id) {
                u.signing_key = Some(pubkey.to_string());
            }
            Ok(())
        }
    }

    // ── In-memory TokenStore ─────────────────────────────────────────────────

    #[derive(Default)]
    struct MemTokenStore {
        tokens: Mutex<Vec<ApiToken>>,
    }

    #[async_trait]
    impl TokenStore for MemTokenStore {
        async fn create(&self, user_id: Uuid, name: &str, token_hash: &str) -> Result<ApiToken> {
            let t = ApiToken {
                id: Uuid::new_v4(),
                user_id,
                name: name.to_string(),
                token_hash: token_hash.to_string(),
                created_at: Utc::now(),
                last_used_at: None,
            };
            self.tokens.lock().unwrap().push(t.clone());
            Ok(t)
        }

        async fn find_by_hash(&self, token_hash: &str) -> Result<Option<ApiToken>> {
            let mut tokens = self.tokens.lock().unwrap();
            if let Some(t) = tokens.iter_mut().find(|t| t.token_hash == token_hash) {
                t.last_used_at = Some(Utc::now());
                return Ok(Some(t.clone()));
            }
            Ok(None)
        }

        async fn list_for_user(&self, user_id: Uuid) -> Result<Vec<ApiToken>> {
            Ok(self
                .tokens
                .lock()
                .unwrap()
                .iter()
                .filter(|t| t.user_id == user_id)
                .cloned()
                .collect())
        }

        async fn delete(&self, id: Uuid, user_id: Uuid) -> Result<bool> {
            let mut tokens = self.tokens.lock().unwrap();
            let before = tokens.len();
            tokens.retain(|t| !(t.id == id && t.user_id == user_id));
            Ok(tokens.len() < before)
        }
    }

    // ── In-memory RefreshTokenStore ──────────────────────────────────────────

    #[derive(Default)]
    struct MemRefreshTokenStore {
        tokens: Mutex<Vec<RefreshToken>>,
    }

    #[async_trait]
    impl RefreshTokenStore for MemRefreshTokenStore {
        async fn create(
            &self,
            user_id: Uuid,
            token_hash: &str,
            expires_at: DateTime<Utc>,
        ) -> Result<RefreshToken> {
            let t = RefreshToken {
                id: Uuid::new_v4(),
                user_id,
                token_hash: token_hash.to_string(),
                created_at: Utc::now(),
                expires_at,
            };
            self.tokens.lock().unwrap().push(t.clone());
            Ok(t)
        }

        async fn find_by_hash(&self, token_hash: &str) -> Result<Option<RefreshToken>> {
            Ok(self
                .tokens
                .lock()
                .unwrap()
                .iter()
                .find(|t| t.token_hash == token_hash)
                .cloned())
        }

        async fn delete_by_hash(&self, token_hash: &str) -> Result<bool> {
            let mut tokens = self.tokens.lock().unwrap();
            let before = tokens.len();
            tokens.retain(|t| t.token_hash != token_hash);
            Ok(tokens.len() < before)
        }

        async fn delete_expired(&self) -> Result<u64> {
            let now = Utc::now();
            let mut tokens = self.tokens.lock().unwrap();
            let before = tokens.len();
            tokens.retain(|t| t.expires_at > now);
            Ok((before - tokens.len()) as u64)
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn make_service() -> AuthService {
        AuthService::new(
            Arc::new(MemUserStore::default()),
            Arc::new(MemTokenStore::default()),
            Arc::new(MemRefreshTokenStore::default()),
            "test-secret-32-chars-long-enough!".to_string(),
        )
    }

    // ── AuthService tests ────────────────────────────────────────────────────

    #[tokio::test]
    async fn register_returns_jwt_and_refresh_token() {
        let svc = make_service();
        let result = svc
            .register("alice@example.com", "password123")
            .await
            .unwrap();
        assert!(result.access_token.len() > 20, "access token should be non-trivially long");
        assert!(!result.access_token.starts_with("nxt_"), "should return a JWT, not an API token");
        assert!(result.refresh_token.starts_with("nxr_"), "refresh token should have nxr_ prefix");
    }

    #[tokio::test]
    async fn register_duplicate_email_fails() {
        let svc = make_service();
        svc.register("alice@example.com", "pass").await.unwrap();
        let err = svc
            .register("alice@example.com", "other")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already registered"));
    }

    #[tokio::test]
    async fn login_returns_jwt_and_refresh_token() {
        let svc = make_service();
        svc.register("bob@example.com", "correcthorsebattery")
            .await
            .unwrap();
        let result = svc
            .login("bob@example.com", "correcthorsebattery")
            .await
            .unwrap();
        assert!(!result.access_token.is_empty());
        assert!(result.refresh_token.starts_with("nxr_"));
    }

    #[tokio::test]
    async fn login_wrong_password_fails() {
        let svc = make_service();
        svc.register("carol@example.com", "right").await.unwrap();
        let err = svc.login("carol@example.com", "wrong").await.unwrap_err();
        assert!(err.to_string().contains("invalid"));
    }

    #[tokio::test]
    async fn login_unknown_email_fails() {
        let svc = make_service();
        let err = svc.login("nobody@example.com", "pass").await.unwrap_err();
        assert!(err.to_string().contains("invalid"));
    }

    #[tokio::test]
    async fn verify_jwt_round_trip() {
        let svc = make_service();
        let result = svc.register("dave@example.com", "pass").await.unwrap();
        let user_id = svc.verify_token(&result.access_token).await.unwrap();
        assert!(!user_id.is_nil());
    }

    #[tokio::test]
    async fn verify_invalid_jwt_fails() {
        let svc = make_service();
        let err = svc.verify_token("not.a.real.jwt").await.unwrap_err();
        assert!(err.to_string().contains("invalid token"));
    }

    // ── S01: Refresh token tests ─────────────────────────────────────────────

    #[tokio::test]
    async fn refresh_access_issues_new_jwt() {
        let svc = make_service();
        let result = svc.register("refresh@example.com", "pass").await.unwrap();
        let new_jwt = svc.refresh_access(&result.refresh_token).await.unwrap();
        // New JWT must be decodable and refer to the same user
        let uid_original = svc.verify_token(&result.access_token).await.unwrap();
        let uid_refreshed = svc.verify_token(&new_jwt).await.unwrap();
        assert_eq!(uid_original, uid_refreshed);
    }

    #[tokio::test]
    async fn refresh_with_invalid_token_fails() {
        let svc = make_service();
        let err = svc.refresh_access("nxr_not_a_real_token").await.unwrap_err();
        assert!(err.to_string().contains("invalid or expired"));
    }

    #[tokio::test]
    async fn logout_revokes_refresh_token() {
        let svc = make_service();
        let result = svc.register("logout@example.com", "pass").await.unwrap();
        svc.logout(&result.refresh_token).await.unwrap();
        let err = svc.refresh_access(&result.refresh_token).await.unwrap_err();
        assert!(err.to_string().contains("invalid or expired"));
    }

    // ── API token tests ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn api_token_create_list_revoke() {
        let svc = make_service();
        let result = svc.register("eve@example.com", "pass").await.unwrap();
        let user_id = svc.verify_token(&result.access_token).await.unwrap();

        let (raw, record) = svc.create_api_token(user_id, "ci-token").await.unwrap();
        assert!(raw.starts_with("nxt_"));
        assert_eq!(record.name, "ci-token");
        assert_eq!(record.user_id, user_id);

        let verified = svc.verify_token(&raw).await.unwrap();
        assert_eq!(verified, user_id);

        let tokens = svc.list_api_tokens(user_id).await.unwrap();
        assert_eq!(tokens.len(), 1);

        let deleted = svc.revoke_api_token(record.id, user_id).await.unwrap();
        assert!(deleted);
        let err = svc.verify_token(&raw).await.unwrap_err();
        assert!(err.to_string().contains("invalid token"));

        let tokens = svc.list_api_tokens(user_id).await.unwrap();
        assert!(tokens.is_empty());
    }

    // ── Email validation ─────────────────────────────────────────────────────

    #[test]
    fn valid_email_accepts_normal_addresses() {
        assert!(valid_email("alice@example.com"));
        assert!(valid_email("user.name+tag@sub.domain.org"));
        assert!(valid_email("x@y.z"));
    }

    #[test]
    fn valid_email_rejects_bad_inputs() {
        assert!(!valid_email("notanemail"));
        assert!(!valid_email("@nodomain.com"));
        assert!(!valid_email("noatsign"));
        assert!(!valid_email("missing@dot"));
        assert!(!valid_email(""));
        assert!(!valid_email(&format!("{}@b.c", "a".repeat(252))));
    }

    #[tokio::test]
    async fn register_invalid_email_rejected() {
        let svc = make_service();
        let err = svc.register("notanemail", "pass").await.unwrap_err();
        assert!(err.to_string().contains("invalid email"));
    }

    // ── JWT duration ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn jwt_expiry_is_approximately_7_days() {
        let svc = make_service();
        let result = svc.register("exp@example.com", "pass").await.unwrap();
        let key = jsonwebtoken::DecodingKey::from_secret(b"test-secret-32-chars-long-enough!");
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
        validation.validate_exp = false;
        let data =
            jsonwebtoken::decode::<super::Claims>(&result.access_token, &key, &validation).unwrap();
        let exp = data.claims.exp;
        let now = chrono::Utc::now().timestamp();
        let diff_secs = exp - now;
        assert!(diff_secs > 6 * 24 * 3600 - 60, "expiry should be ~7 days, got {diff_secs}s");
        assert!(diff_secs < 8 * 24 * 3600, "expiry should be ~7 days, got {diff_secs}s");
    }

    #[tokio::test]
    async fn revoke_other_users_token_fails() {
        let svc = make_service();
        let r1 = svc.register("frank@example.com", "pass").await.unwrap();
        let r2 = svc.register("grace@example.com", "pass").await.unwrap();
        let uid1 = svc.verify_token(&r1.access_token).await.unwrap();
        let uid2 = svc.verify_token(&r2.access_token).await.unwrap();

        let (_, record) = svc.create_api_token(uid1, "my-token").await.unwrap();
        let deleted = svc.revoke_api_token(record.id, uid2).await.unwrap();
        assert!(!deleted, "should not be able to revoke another user's token");
    }
}
