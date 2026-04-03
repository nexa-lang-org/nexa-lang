use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;

use crate::domain::{
    package::{Package, PackageVersion},
    token::ApiToken,
    user::User,
};

#[async_trait]
pub trait UserStore: Send + Sync {
    async fn create(&self, email: &str, password_hash: &str) -> Result<User>;
    async fn find_by_email(&self, email: &str) -> Result<Option<User>>;
}

#[async_trait]
pub trait TokenStore: Send + Sync {
    /// Persist a new token (only the hash is stored, not the raw value).
    async fn create(&self, user_id: Uuid, name: &str, token_hash: &str) -> Result<ApiToken>;
    /// Look up a token by its hash; also bumps `last_used_at`.
    async fn find_by_hash(&self, token_hash: &str) -> Result<Option<ApiToken>>;
    /// List all tokens for a user (hashes are not returned to the client).
    async fn list_for_user(&self, user_id: Uuid) -> Result<Vec<ApiToken>>;
    /// Delete a token by its id, scoped to the owner.
    async fn delete(&self, id: Uuid, user_id: Uuid) -> Result<bool>;
}

#[async_trait]
pub trait PackageStore: Send + Sync {
    async fn find_or_create_package(&self, name: &str, owner_id: Uuid) -> Result<Package>;
    async fn find_package(&self, name: &str) -> Result<Option<Package>>;
    async fn publish_version(
        &self,
        pkg_id: Uuid,
        version: &str,
        bundle: &[u8],
        manifest: &str,
        signature: &str,
    ) -> Result<PackageVersion>;
    async fn get_version(&self, name: &str, version: &str) -> Result<Option<PackageVersion>>;
    async fn get_latest_version(&self, name: &str) -> Result<Option<PackageVersion>>;
    async fn list_versions(&self, name: &str) -> Result<Vec<PackageVersion>>;
    async fn search(&self, q: &str, page: i64, per_page: i64) -> Result<Vec<Package>>;
}
