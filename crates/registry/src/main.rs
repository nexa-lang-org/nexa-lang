use std::sync::Arc;

use nexa_registry::{
    application::services::{auth::AuthService, packages::PackagesService},
    infrastructure::postgres::{PgPackageStore, PgUserStore},
    interfaces::http::{build_router, AppState},
};
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");
    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "4000".into())
        .parse()
        .expect("PORT must be a number");

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;

    // Run migrations
    sqlx::migrate!("./migrations").run(&pool).await?;

    println!("Nexa Registry → http://0.0.0.0:{port}");

    let user_store = Arc::new(PgUserStore::new(pool.clone()));
    let package_store = Arc::new(PgPackageStore::new(pool));

    let state = AppState {
        auth: Arc::new(AuthService::new(user_store, jwt_secret)),
        packages: Arc::new(PackagesService::new(package_store)),
    };

    let router = build_router(state);
    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, router.into_make_service()).await?;
    Ok(())
}
