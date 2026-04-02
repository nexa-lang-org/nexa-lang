use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use uuid::Uuid;

use crate::application::ports::storage::PackageStore;
use crate::domain::package::{Package, PackageVersion};

pub struct PackagesService {
    store: Arc<dyn PackageStore>,
}

impl PackagesService {
    pub fn new(store: Arc<dyn PackageStore>) -> Self {
        Self { store }
    }

    /// Publish a .nexa bundle for a given package name + version.
    /// Validates the embedded signature before storing.
    pub async fn publish(
        &self,
        name: &str,
        owner_id: Uuid,
        bundle_bytes: Vec<u8>,
    ) -> Result<PackageVersion> {
        // Extract manifest.json and signature.sig from the ZIP
        let (manifest, signature) = extract_manifest_and_sig(&bundle_bytes)?;

        // Verify signature: SHA-256(nxb || manifest bytes)
        let nxb = extract_nxb(&bundle_bytes)?;
        let mut hasher = Sha256::new();
        hasher.update(&nxb);
        hasher.update(manifest.as_bytes());
        let computed = format!("{:x}", hasher.finalize());
        if computed != signature.trim() {
            return Err(anyhow!("bundle signature verification failed"));
        }

        // Parse version from manifest
        let version = parse_version(&manifest)?;

        let pkg = self.store.find_or_create_package(name, owner_id).await?;

        // Check for duplicate version
        if self.store.get_version(name, &version).await?.is_some() {
            return Err(anyhow!("version {version} already published for {name}"));
        }

        self.store
            .publish_version(pkg.id, &version, &bundle_bytes, &manifest, &signature)
            .await
    }

    pub async fn get_package(&self, name: &str) -> Result<Option<Package>> {
        self.store.find_package(name).await
    }

    pub async fn list_versions(&self, name: &str) -> Result<Vec<PackageVersion>> {
        self.store.list_versions(name).await
    }

    pub async fn download(&self, name: &str, version: &str) -> Result<Option<PackageVersion>> {
        if version == "latest" {
            self.store.get_latest_version(name).await
        } else {
            self.store.get_version(name, version).await
        }
    }

    pub async fn search(&self, q: &str, page: i64, per_page: i64) -> Result<Vec<Package>> {
        self.store.search(q, page, per_page).await
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn extract_nxb(bundle: &[u8]) -> Result<Vec<u8>> {
    extract_zip_entry(bundle, "app.nxb")
}

fn extract_manifest_and_sig(bundle: &[u8]) -> Result<(String, String)> {
    let manifest_bytes = extract_zip_entry(bundle, "manifest.json")?;
    let sig_bytes = extract_zip_entry(bundle, "signature.sig")?;
    let manifest = String::from_utf8(manifest_bytes)
        .map_err(|_| anyhow!("manifest.json is not valid UTF-8"))?;
    let signature =
        String::from_utf8(sig_bytes).map_err(|_| anyhow!("signature.sig is not valid UTF-8"))?;
    Ok((manifest, signature))
}

fn extract_zip_entry(bundle: &[u8], name: &str) -> Result<Vec<u8>> {
    use std::io::{Cursor, Read};
    let cursor = Cursor::new(bundle);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| anyhow!("invalid ZIP: {e}"))?;
    let mut entry = archive
        .by_name(name)
        .map_err(|_| anyhow!("bundle missing '{name}'"))?;
    let mut buf = Vec::new();
    entry
        .read_to_end(&mut buf)
        .map_err(|e| anyhow!("read '{name}': {e}"))?;
    Ok(buf)
}

fn parse_version(manifest: &str) -> Result<String> {
    let v: serde_json::Value =
        serde_json::from_str(manifest).map_err(|e| anyhow!("invalid manifest JSON: {e}"))?;
    v["version"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("manifest missing 'version' field"))
}
