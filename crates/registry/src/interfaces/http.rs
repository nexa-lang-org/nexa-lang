use axum::{
    async_trait,
    body::Bytes,
    extract::{FromRequestParts, Multipart, Path, Query, State},
    http::{header, request::Parts, HeaderMap, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_governor::{governor::GovernorConfigBuilder, GovernorLayer};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

use crate::application::services::{auth::AuthService, packages::PackagesService};

// ── Auth extractor ────────────────────────────────────────────────────────────

/// Axum extractor that validates a Bearer token and resolves the caller's user ID.
/// Eliminates the repeated 6-line token-check block from every protected handler.
struct AuthUser(uuid::Uuid);

#[async_trait]
impl FromRequestParts<AppState> for AuthUser {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_bearer(&parts.headers)
            .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "missing Authorization header"))?;
        let user_id = state
            .auth
            .verify_token(&token)
            .await
            .map_err(|_| err(StatusCode::UNAUTHORIZED, "invalid or expired token"))?;
        Ok(AuthUser(user_id))
    }
}

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum accepted bundle size for package uploads (50 MB).
const MAX_BUNDLE_BYTES: usize = 50 * 1024 * 1024;

// ── App state ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<AuthService>,
    pub packages: Arc<PackagesService>,
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn build_router(state: AppState) -> Router {
    // Rate-limit register + login: 1 token replenished every 6 s (≈ 10 req/min),
    // initial burst of 3. Keyed per IP to block brute-force / enumeration.
    let auth_limiter = Arc::new(
        GovernorConfigBuilder::default()
            .per_millisecond(6_000)
            .burst_size(3)
            .finish()
            .expect("valid governor config"),
    );
    let auth_routes = Router::new()
        .route("/v1/auth/register", post(register))
        .route("/v1/auth/login", post(login))
        .layer(GovernorLayer {
            config: auth_limiter,
        });

    // CORS: public registry — allow any origin, restrict methods and headers.
    let cors = CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);

    let v1 = Router::new()
        .route("/v1/auth/tokens", post(create_token).get(list_tokens))
        .route("/v1/auth/tokens/:id", axum::routing::delete(revoke_token))
        .route("/v1/packages", get(list_packages))
        .route("/v1/packages/:name", get(get_package))
        .route("/v1/packages/:name/publish", post(publish))
        .route("/v1/packages/:name/:version/download", get(download))
        .route("/v1/packages/:name/:version/source", get(get_source));

    Router::new()
        .merge(auth_routes)
        .merge(v1)
        // Health and UI are unversioned
        .route("/health", get(health))
        .route("/ui/packages/:name", get(ui_package))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

// ── Request / response types ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct AuthBody {
    email: String,
    password: String,
}

#[derive(Serialize)]
struct TokenResponse {
    token: String,
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
    #[serde(default = "default_page")]
    page: i64,
    #[serde(default = "default_per_page")]
    per_page: i64,
}

fn default_page() -> i64 {
    1
}
fn default_per_page() -> i64 {
    20
}

#[derive(Serialize)]
struct PackageInfo {
    name: String,
    versions: Vec<VersionInfo>,
}

#[derive(Serialize)]
struct VersionInfo {
    version: String,
    published_at: String,
}

#[derive(Serialize)]
struct PackageListItem {
    name: String,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

fn err(status: StatusCode, msg: &str) -> Response {
    (status, Json(serde_json::json!({ "error": msg }))).into_response()
}

/// Package names: 1–214 chars, start with alphanumeric, only `a-z A-Z 0-9 - _ .`,
/// no consecutive dots (prevents `..` path traversal).
fn valid_package_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 214
        && name
            .chars()
            .next()
            .map(|c| c.is_alphanumeric())
            .unwrap_or(false)
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '.')
        && !name.contains("..")
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn register(State(state): State<AppState>, Json(body): Json<AuthBody>) -> Response {
    match state.auth.register(&body.email, &body.password).await {
        Ok(token) => {
            tracing::info!(email = %body.email, "User registered");
            (StatusCode::CREATED, Json(TokenResponse { token })).into_response()
        }
        Err(e) => {
            let detail = e.to_string();
            tracing::warn!(email = %body.email, error = %detail, "Registration failed");
            // Expose only known-safe domain messages; suppress all internal details.
            let safe = if detail.contains("email already registered") || detail.contains("invalid email") {
                detail.as_str()
            } else {
                "registration failed"
            };
            err(StatusCode::BAD_REQUEST, safe)
        }
    }
}

async fn login(State(state): State<AppState>, Json(body): Json<AuthBody>) -> Response {
    match state.auth.login(&body.email, &body.password).await {
        Ok(token) => {
            tracing::debug!(email = %body.email, "User logged in");
            Json(TokenResponse { token }).into_response()
        }
        Err(e) => {
            // Log the real cause internally; never reveal details to the caller.
            tracing::warn!(email = %body.email, error = %e, "Login failed");
            err(StatusCode::UNAUTHORIZED, "invalid credentials")
        }
    }
}

async fn publish(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    headers: HeaderMap,
    Path(name): Path<String>,
    mut multipart: Multipart,
) -> Response {
    if !valid_package_name(&name) {
        return err(
            StatusCode::BAD_REQUEST,
            "invalid package name: use letters, digits, hyphens, underscores or dots (max 214 chars)",
        );
    }

    // Require Ed25519 signing headers
    let ed25519_pubkey = match headers
        .get("x-nexa-signing-key")
        .and_then(|v| v.to_str().ok())
    {
        Some(v) => v.to_string(),
        None => return err(StatusCode::BAD_REQUEST, "missing X-Nexa-Signing-Key header"),
    };
    let ed25519_sig = match headers
        .get("x-nexa-signature")
        .and_then(|v| v.to_str().ok())
    {
        Some(v) => v.to_string(),
        None => return err(StatusCode::BAD_REQUEST, "missing X-Nexa-Signature header"),
    };

    // Read the first multipart field (the .nexa bundle)
    let bundle_bytes = match multipart.next_field().await {
        Ok(Some(field)) => match field.bytes().await {
            Ok(b) => b.to_vec(),
            Err(e) => return err(StatusCode::BAD_REQUEST, &format!("read field: {e}")),
        },
        Ok(None) => return err(StatusCode::BAD_REQUEST, "no file in multipart"),
        Err(e) => return err(StatusCode::BAD_REQUEST, &format!("multipart error: {e}")),
    };

    if bundle_bytes.len() > MAX_BUNDLE_BYTES {
        return err(
            StatusCode::PAYLOAD_TOO_LARGE,
            "bundle exceeds the 50 MB upload limit",
        );
    }

    let bundle_size = bundle_bytes.len();
    match state
        .packages
        .publish(&name, user_id, bundle_bytes, &ed25519_pubkey, &ed25519_sig)
        .await
    {
        Ok(v) => {
            tracing::info!(
                package = %name,
                version = %v.version,
                size_bytes = bundle_size,
                "Package published"
            );
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "name": name,
                    "version": v.version,
                    "published_at": v.published_at.to_rfc3339(),
                })),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!(package = %name, error = %e, "Publish failed");
            err(StatusCode::BAD_REQUEST, &e.to_string())
        }
    }
}

async fn get_package(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    if !valid_package_name(&name) {
        return err(StatusCode::BAD_REQUEST, "invalid package name");
    }
    let pkg = match state.packages.get_package(&name).await {
        Ok(Some(p)) => p,
        Ok(None) => return err(StatusCode::NOT_FOUND, "package not found"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let versions = match state.packages.list_versions(&name).await {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let info = PackageInfo {
        name: pkg.name,
        versions: versions
            .iter()
            .map(|v| VersionInfo {
                version: v.version.clone(),
                published_at: v.published_at.to_rfc3339(),
            })
            .collect(),
    };
    Json(info).into_response()
}

async fn download(
    State(state): State<AppState>,
    Path((name, version)): Path<(String, String)>,
) -> Response {
    if !valid_package_name(&name) {
        return err(StatusCode::BAD_REQUEST, "invalid package name");
    }
    match state.packages.download(&name, &version).await {
        Ok(Some(pv)) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/octet-stream"),
                (
                    header::CONTENT_DISPOSITION,
                    &format!("attachment; filename=\"{name}-{}.nexa\"", pv.version),
                ),
            ],
            Bytes::from(pv.bundle),
        )
            .into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "version not found"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn list_packages(State(state): State<AppState>, Query(q): Query<SearchQuery>) -> Response {
    match state.packages.search(&q.q, q.page, q.per_page).await {
        Ok(pkgs) => {
            let items: Vec<PackageListItem> = pkgs
                .into_iter()
                .map(|p| PackageListItem { name: p.name })
                .collect();
            Json(items).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// Extract the first `src/*.nx` file from a stored bundle (ZIP) and return its
/// text content. Returns 404 if the version or source entry is not found.
async fn get_source(
    State(state): State<AppState>,
    Path((name, version)): Path<(String, String)>,
) -> Response {
    if !valid_package_name(&name) {
        return err(StatusCode::BAD_REQUEST, "invalid package name");
    }
    let pv = match state.packages.download(&name, &version).await {
        Ok(Some(pv)) => pv,
        Ok(None) => return err(StatusCode::NOT_FOUND, "version not found"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    match extract_source_from_bundle(&pv.bundle) {
        Some(src) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            src,
        )
            .into_response(),
        None => err(StatusCode::NOT_FOUND, "no source included in this bundle"),
    }
}

/// Simple HTML web page showing package info and source code.
async fn ui_package(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    if !valid_package_name(&name) {
        return err(StatusCode::BAD_REQUEST, "invalid package name");
    }
    let pkg = match state.packages.get_package(&name).await {
        Ok(Some(p)) => p,
        Ok(None) => return err(StatusCode::NOT_FOUND, "package not found"),
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let versions = match state.packages.list_versions(&name).await {
        Ok(v) => v,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    // Fetch source of the latest version, if available
    let latest_source: Option<String> = if let Some(latest) = versions.last() {
        state
            .packages
            .download(&name, &latest.version)
            .await
            .ok()
            .flatten()
            .and_then(|pv| extract_source_from_bundle(&pv.bundle))
    } else {
        None
    };

    let versions_html: String = versions
        .iter()
        .rev()
        .map(|v| {
            format!(
                "<tr><td>{}</td><td>{}</td><td><a href=\"/packages/{}/{}/download\">\
                 download</a> &nbsp; <a href=\"/packages/{}/{}/source\">source</a></td></tr>",
                v.version,
                v.published_at.format("%Y-%m-%d %H:%M UTC"),
                pkg.name,
                v.version,
                pkg.name,
                v.version,
            )
        })
        .collect();

    let source_section = match latest_source {
        Some(src) => format!(
            "<section class=\"src\"><h2>Source — latest version</h2>\
             <pre><code>{}</code></pre></section>",
            html_escape(&src)
        ),
        None => "<p class=\"dim\">No source available for this package.</p>".to_string(),
    };

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width,initial-scale=1" />
  <title>{name} — Nexa Registry</title>
  <style>
    *,*::before,*::after{{box-sizing:border-box;margin:0;padding:0}}
    body{{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif;
          background:#0f0f13;color:#e2e2e8;padding:2rem 1.5rem;max-width:900px;margin:0 auto}}
    h1{{font-size:2rem;margin-bottom:.25rem;color:#a78bfa}}
    h2{{font-size:1.1rem;margin:1.5rem 0 .75rem;color:#94a3b8}}
    a{{color:#7dd3fc;text-decoration:none}}a:hover{{text-decoration:underline}}
    table{{border-collapse:collapse;width:100%;margin-bottom:1.5rem}}
    th,td{{padding:.5rem 1rem;text-align:left;border-bottom:1px solid #1e1e2e}}
    th{{color:#94a3b8;font-weight:600;font-size:.85rem}}
    pre{{background:#1e1e2e;border-radius:8px;padding:1.25rem;overflow-x:auto;
         font-size:.85rem;line-height:1.6;border:1px solid #2e2e3e}}
    code{{color:#cdd6f4;white-space:pre}}
    .dim{{color:#555;font-style:italic;margin-top:1rem}}
    nav{{margin-bottom:2rem;color:#555}}
    nav a{{color:#7dd3fc}}
    .badge{{display:inline-block;background:#1e1e2e;border:1px solid #2e2e3e;
            padding:.15rem .6rem;border-radius:99px;font-size:.8rem;color:#94a3b8}}
    .install{{background:#1e1e2e;border:1px solid #2e2e3e;border-radius:8px;
              padding:1rem 1.25rem;font-family:monospace;font-size:.9rem;
              color:#a8ff78;margin:1rem 0 1.5rem}}
  </style>
</head>
<body>
  <nav><a href="/">Nexa Registry</a> / {name}</nav>
  <h1>{name}</h1>
  <p class="badge">{count} version(s)</p>
  <div class="install">nexa install {name}</div>
  <h2>Versions</h2>
  <table>
    <thead><tr><th>Version</th><th>Published</th><th>Links</th></tr></thead>
    <tbody>{versions_html}</tbody>
  </table>
  {source_section}
</body>
</html>"#,
        name = pkg.name,
        count = versions.len(),
    );

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

// ── API token handlers ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateTokenBody {
    name: String,
}

#[derive(Serialize)]
struct CreatedTokenResponse {
    id: String,
    name: String,
    token: String, // shown only once
    created_at: String,
}

#[derive(Serialize)]
struct TokenListItem {
    id: String,
    name: String,
    created_at: String,
    last_used_at: Option<String>,
}

async fn create_token(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Json(body): Json<CreateTokenBody>,
) -> Response {
    match state.auth.create_api_token(user_id, &body.name).await {
        Ok((raw, record)) => {
            tracing::info!(user_id = %user_id, name = %body.name, "API token created");
            (
                StatusCode::CREATED,
                Json(CreatedTokenResponse {
                    id: record.id.to_string(),
                    name: record.name,
                    token: raw,
                    created_at: record.created_at.to_rfc3339(),
                }),
            )
                .into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn list_tokens(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
) -> Response {
    match state.auth.list_api_tokens(user_id).await {
        Ok(tokens) => {
            let items: Vec<TokenListItem> = tokens
                .into_iter()
                .map(|t| TokenListItem {
                    id: t.id.to_string(),
                    name: t.name,
                    created_at: t.created_at.to_rfc3339(),
                    last_used_at: t.last_used_at.map(|d| d.to_rfc3339()),
                })
                .collect();
            Json(items).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn revoke_token(
    State(state): State<AppState>,
    AuthUser(user_id): AuthUser,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    match state.auth.revoke_api_token(id, user_id).await {
        Ok(true) => {
            tracing::info!(token_id = %id, user_id = %user_id, "API token revoked");
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => err(StatusCode::NOT_FOUND, "token not found"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

// ── Bundle helpers ────────────────────────────────────────────────────────────

/// Extract the source file (`src/*.nx`) from a raw `.nexa` bundle (ZIP bytes).
fn extract_source_from_bundle(bundle: &[u8]) -> Option<String> {
    use std::io::{Cursor, Read};
    let cursor = Cursor::new(bundle);
    let mut archive = zip::ZipArchive::new(cursor).ok()?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).ok()?;
        let name = entry.name().to_string();
        if name.starts_with("src/") && name.ends_with(".nx") {
            let mut buf = String::new();
            entry.read_to_string(&mut buf).ok()?;
            return Some(buf);
        }
    }
    None
}

/// Minimal HTML escaping to safely embed source code in a <pre> block.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
