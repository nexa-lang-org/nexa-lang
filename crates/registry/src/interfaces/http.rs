use axum::{
    body::Bytes,
    extract::{Multipart, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::trace::TraceLayer;

use crate::application::services::{auth::AuthService, packages::PackagesService};

// ── App state ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<AuthService>,
    pub packages: Arc<PackagesService>,
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/packages", get(list_packages))
        .route("/packages/:name", get(get_package))
        .route("/packages/:name/publish", post(publish))
        .route("/packages/:name/:version/download", get(download))
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
            tracing::warn!(email = %body.email, error = %e, "Registration failed");
            err(StatusCode::BAD_REQUEST, &e.to_string())
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
            tracing::warn!(email = %body.email, error = %e, "Login failed");
            err(StatusCode::UNAUTHORIZED, &e.to_string())
        }
    }
}

async fn publish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    mut multipart: Multipart,
) -> Response {
    let token = match extract_bearer(&headers) {
        Some(t) => t,
        None => {
            tracing::warn!(package = %name, "Publish rejected: missing token");
            return err(StatusCode::UNAUTHORIZED, "missing Authorization header");
        }
    };
    let user_id = match state.auth.verify_token(&token) {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(package = %name, error = %e, "Publish rejected: invalid token");
            return err(StatusCode::UNAUTHORIZED, &e.to_string());
        }
    };

    // Read the first multipart field (the .nexa file)
    let bundle_bytes = match multipart.next_field().await {
        Ok(Some(field)) => match field.bytes().await {
            Ok(b) => b.to_vec(),
            Err(e) => return err(StatusCode::BAD_REQUEST, &format!("read field: {e}")),
        },
        Ok(None) => return err(StatusCode::BAD_REQUEST, "no file in multipart"),
        Err(e) => return err(StatusCode::BAD_REQUEST, &format!("multipart error: {e}")),
    };

    let bundle_size = bundle_bytes.len();
    match state.packages.publish(&name, user_id, bundle_bytes).await {
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
