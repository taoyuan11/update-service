mod auth;
mod config;
mod error;
mod models;
mod routes;
mod storage;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::DefaultBodyLimit;
use config::Config;
use error::ApiError;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower_http::{limit::RequestBodyLimitLayer, trace::TraceLayer};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub config: Arc<Config>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "update_service_api=info,tower_http=info".into()),
        )
        .init();
    let config = Config::from_env().map_err(std::io::Error::other)?;
    tokio::fs::create_dir_all(&config.temp_dir).await?;
    let db = PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await?;
    sqlx::migrate!("./migrations").run(&db).await?;
    bootstrap_admin(&db, &config).await?;

    let max_upload = config.upload_max_bytes;
    let addr = config.bind_addr;
    let state = AppState {
        db,
        config: Arc::new(config),
    };
    routes::recover_migration_jobs(&state).await?;
    let cleanup_state = state.clone();
    tokio::spawn(async move {
        loop {
            if let Err(error) = routes::process_cleanup_jobs(&cleanup_state).await {
                tracing::error!(%error, "storage cleanup worker failed");
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
    let migration_state = state.clone();
    tokio::spawn(async move {
        loop {
            if let Err(error) = routes::process_migration_jobs(&migration_state).await {
                tracing::error!(%error, "storage migration worker failed");
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    let migration_maintenance_state = state.clone();
    tokio::spawn(async move {
        loop {
            if let Err(error) = routes::purge_old_migrations(&migration_maintenance_state).await {
                tracing::error!(%error, "storage migration retention cleanup failed");
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
    let app = routes::router(state)
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(max_upload))
        .layer(TraceLayer::new_for_http());
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "update service listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn bootstrap_admin(db: &PgPool, config: &Config) -> Result<(), ApiError> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(db)
        .await?;
    if count > 0 {
        return Ok(());
    }
    let username = config.initial_admin_username.as_deref().ok_or_else(|| {
        ApiError::Internal("INITIAL_ADMIN_USERNAME is required when no users exist".into())
    })?;
    let password = config.initial_admin_password.as_deref().ok_or_else(|| {
        ApiError::Internal("INITIAL_ADMIN_PASSWORD is required when no users exist".into())
    })?;
    if username.len() < 3
        || !username
            .bytes()
            .all(|v| v.is_ascii_alphanumeric() || matches!(v, b'_' | b'-'))
    {
        return Err(ApiError::Internal(
            "INITIAL_ADMIN_USERNAME is invalid".into(),
        ));
    }
    if password.is_empty() {
        return Err(ApiError::Internal(
            "INITIAL_ADMIN_PASSWORD must not be empty".into(),
        ));
    }
    let hash = auth::password_hash(password)?;
    sqlx::query("INSERT INTO users (id,username,password_hash,role) VALUES ($1,$2,$3,'admin')")
        .bind(Uuid::new_v4())
        .bind(username)
        .bind(hash)
        .execute(db)
        .await?;
    tracing::info!(username, "initial administrator created");
    Ok(())
}
