use std::sync::Arc;

use nexa_registry::{
    application::services::{auth::AuthService, packages::PackagesService},
    infrastructure::postgres::{PgPackageStore, PgUserStore},
    interfaces::http::{build_router, AppState},
};
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialise le subscriber tracing.
    // Niveau par défaut : INFO. Surcharger avec RUST_LOG (ex: RUST_LOG=debug,sqlx=warn).
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

    let user_store = Arc::new(PgUserStore::new(pool.clone()));
    let package_store = Arc::new(PgPackageStore::new(pool));

    let state = AppState {
        auth: Arc::new(AuthService::new(user_store, jwt_secret)),
        packages: Arc::new(PackagesService::new(package_store)),
    };

    let router = build_router(state);
    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    tracing::info!(address = %addr, "Nexa Registry listening");
    axum::serve(listener, router.into_make_service()).await?;
    Ok(())
}
