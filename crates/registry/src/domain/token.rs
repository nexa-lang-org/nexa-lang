use chrono::{DateTime, Utc};
use uuid::Uuid;

/// A permanent API token belonging to a user.
/// The raw token value is only shown once at creation time;
/// only its SHA-256 hash is persisted in the database.
#[derive(Debug, Clone)]
pub struct ApiToken {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}
