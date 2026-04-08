use chrono::{DateTime, Utc};
use uuid::Uuid;

/// A long-lived refresh token used to obtain new access tokens without
/// re-authenticating with a password.
/// Only the SHA-256 hash of the raw value is persisted.
#[derive(Debug, Clone)]
pub struct RefreshToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}
