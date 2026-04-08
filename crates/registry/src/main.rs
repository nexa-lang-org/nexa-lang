use std::sync::Arc;

use axum::http::HeaderValue;
use nexa_registry::{
    application::services::{auth::AuthService, packages::PackagesService},
    infrastructure::postgres::{PgPackageStore, PgRefreshTokenStore, PgTokenStore, PgUserStore},
    interfaces::http::{build_router, AppState},
};
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialise the tracing subscriber.
    // Default level: INFO. Override with RUST_LOG (e.g. RUST_LOG=debug,sqlx=warn).
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(tracing::Level::INFO.into())
                .from_env_lossy(),
        )
        .init();

    dotenvy::dotenv().ok();

    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");

    if jwt_secret.len() < 32 {
        tracing::error!(
            len = jwt_secret.len(),
            "JWT_SECRET is too short ({} chars) — minimum 32 required. \
             A short secret is trivially brute-forceable. Aborting.",
            jwt_secret.len()
        );
        std::process::exit(1);
    }

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "4000".into())
        .parse()
        .expect("PORT must be a number");

    tracing::info!("Connecting to database…");
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;

    tracing::info!("Running database migrations…");
    sqlx::migrate!("./migrations").run(&pool).await?;

    // S02: read allowed CORS origins from environment.
    // CORS_ORIGINS=https://app.nexa-lang.org,https://staging.nexa-lang.org
    // If unset, any origin is allowed (suitable for dev / open registries).
    let allowed_origins: Option<Vec<HeaderValue>> =
        std::env::var("CORS_ORIGINS").ok().map(|raw| {
            raw.split(',')
                .filter_map(|s| {
                    let s = s.trim();
                    match HeaderValue::from_str(s) {
                        Ok(v) => Some(v),
                        Err(_) => {
                            tracing::warn!(origin = s, "CORS_ORIGINS: skipping invalid origin");
                            None
                        }
                    }
                })
                .collect()
        });

    if let Some(ref origins) = allowed_origins {
        tracing::info!(count = origins.len(), "CORS restricted to configured origins");
    } else {
        tracing::warn!("CORS_ORIGINS not set — allowing any origin (dev/open mode)");
    }

    let user_store = Arc::new(PgUserStore::new(pool.clone()));
    let token_store = Arc::new(PgTokenStore::new(pool.clone()));
    let refresh_token_store = Arc::new(PgRefreshTokenStore::new(pool.clone()));
    let package_store = Arc::new(PgPackageStore::new(pool));

    let state = AppState {
        auth: Arc::new(AuthService::new(
            user_store.clone(),
            token_store,
            refresh_token_store,
            jwt_secret,
        )),
        packages: Arc::new(PackagesService::new(package_store, user_store)),
    };

    let router = build_router(state, allowed_origins);
    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    tracing::info!(address = %addr, "Nexa Registry listening");
    axum::serve(listener, router.into_make_service()).await?;
    Ok(())
}
