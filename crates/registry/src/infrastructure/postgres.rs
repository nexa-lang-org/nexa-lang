use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::application::ports::storage::{PackageStore, UserStore};
use crate::domain::{
    package::{Package, PackageVersion},
    user::User,
};

// ── Row types (used with query_as, no compile-time DB required) ──────────────

#[derive(FromRow)]
struct UserRow {
    id: Uuid,
    email: String,
    password_hash: String,
    created_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct PackageRow {
    id: Uuid,
    name: String,
    owner_id: Uuid,
    created_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct VersionRow {
    id: Uuid,
    package_id: Uuid,
    version: String,
    bundle: Vec<u8>,
    manifest: serde_json::Value,
    signature: String,
    published_at: DateTime<Utc>,
}

impl From<UserRow> for User {
    fn from(r: UserRow) -> Self {
        User {
            id: r.id,
            email: r.email,
            password_hash: r.password_hash,
            created_at: r.created_at,
        }
    }
}

impl From<PackageRow> for Package {
    fn from(r: PackageRow) -> Self {
        Package {
            id: r.id,
            name: r.name,
            owner_id: r.owner_id,
            created_at: r.created_at,
        }
    }
}

impl From<VersionRow> for PackageVersion {
    fn from(r: VersionRow) -> Self {
        PackageVersion {
            id: r.id,
            package_id: r.package_id,
            version: r.version,
            bundle: r.bundle,
            manifest: r.manifest.to_string(),
            signature: r.signature,
            published_at: r.published_at,
        }
    }
}

// ── UserStore ────────────────────────────────────────────────────────────────

pub struct PgUserStore {
    pool: PgPool,
}

impl PgUserStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl UserStore for PgUserStore {
    async fn create(&self, email: &str, password_hash: &str) -> Result<User> {
        let row = sqlx::query_as::<_, UserRow>(
            "INSERT INTO users (email, password_hash) VALUES ($1, $2)
             RETURNING id, email, password_hash, created_at",
        )
        .bind(email)
        .bind(password_hash)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| anyhow!("create user: {e}"))?;
        Ok(row.into())
    }

    async fn find_by_email(&self, email: &str) -> Result<Option<User>> {
        let row = sqlx::query_as::<_, UserRow>(
            "SELECT id, email, password_hash, created_at FROM users WHERE email = $1",
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| anyhow!("find user: {e}"))?;
        Ok(row.map(Into::into))
    }
}

// ── PackageStore ─────────────────────────────────────────────────────────────

pub struct PgPackageStore {
    pool: PgPool,
}

impl PgPackageStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl PackageStore for PgPackageStore {
    async fn find_or_create_package(&self, name: &str, owner_id: Uuid) -> Result<Package> {
        let row = sqlx::query_as::<_, PackageRow>(
            "INSERT INTO packages (name, owner_id) VALUES ($1, $2)
             ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.name
             RETURNING id, name, owner_id, created_at",
        )
        .bind(name)
        .bind(owner_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| anyhow!("find_or_create package: {e}"))?;
        Ok(row.into())
    }

    async fn find_package(&self, name: &str) -> Result<Option<Package>> {
        let row = sqlx::query_as::<_, PackageRow>(
            "SELECT id, name, owner_id, created_at FROM packages WHERE name = $1",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| anyhow!("find package: {e}"))?;
        Ok(row.map(Into::into))
    }

    async fn publish_version(
        &self,
        pkg_id: Uuid,
        version: &str,
        bundle: &[u8],
        manifest: &str,
        signature: &str,
    ) -> Result<PackageVersion> {
        let manifest_json: serde_json::Value =
            serde_json::from_str(manifest).map_err(|e| anyhow!("invalid manifest: {e}"))?;

        let row = sqlx::query_as::<_, VersionRow>(
            "INSERT INTO package_versions (package_id, version, bundle, manifest, signature)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING id, package_id, version, bundle, manifest, signature, published_at",
        )
        .bind(pkg_id)
        .bind(version)
        .bind(bundle)
        .bind(&manifest_json)
        .bind(signature)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| anyhow!("publish version: {e}"))?;
        Ok(row.into())
    }

    async fn get_version(&self, name: &str, version: &str) -> Result<Option<PackageVersion>> {
        let row = sqlx::query_as::<_, VersionRow>(
            "SELECT pv.id, pv.package_id, pv.version, pv.bundle, pv.manifest,
                    pv.signature, pv.published_at
             FROM package_versions pv
             JOIN packages p ON p.id = pv.package_id
             WHERE p.name = $1 AND pv.version = $2",
        )
        .bind(name)
        .bind(version)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| anyhow!("get version: {e}"))?;
        Ok(row.map(Into::into))
    }

    async fn get_latest_version(&self, name: &str) -> Result<Option<PackageVersion>> {
        let row = sqlx::query_as::<_, VersionRow>(
            "SELECT pv.id, pv.package_id, pv.version, pv.bundle, pv.manifest,
                    pv.signature, pv.published_at
             FROM package_versions pv
             JOIN packages p ON p.id = pv.package_id
             WHERE p.name = $1
             ORDER BY pv.published_at DESC
             LIMIT 1",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| anyhow!("get latest: {e}"))?;
        Ok(row.map(Into::into))
    }

    async fn list_versions(&self, name: &str) -> Result<Vec<PackageVersion>> {
        let rows = sqlx::query_as::<_, VersionRow>(
            "SELECT pv.id, pv.package_id, pv.version, pv.bundle, pv.manifest,
                    pv.signature, pv.published_at
             FROM package_versions pv
             JOIN packages p ON p.id = pv.package_id
             WHERE p.name = $1
             ORDER BY pv.published_at DESC",
        )
        .bind(name)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| anyhow!("list versions: {e}"))?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn search(&self, q: &str, page: i64, per_page: i64) -> Result<Vec<Package>> {
        let pattern = format!("%{q}%");
        let offset = (page - 1).max(0) * per_page;
        let rows = sqlx::query_as::<_, PackageRow>(
            "SELECT id, name, owner_id, created_at FROM packages
             WHERE name ILIKE $1
             ORDER BY name
             LIMIT $2 OFFSET $3",
        )
        .bind(&pattern)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| anyhow!("search: {e}"))?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
}
