use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;

use crate::domain::{
    package::{Package, PackageVersion},
    user::User,
};

#[async_trait]
pub trait UserStore: Send + Sync {
    async fn create(&self, email: &str, password_hash: &str) -> Result<User>;
    async fn find_by_email(&self, email: &str) -> Result<Option<User>>;
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
