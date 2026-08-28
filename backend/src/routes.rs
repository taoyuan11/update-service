use std::{path::PathBuf, time::Duration as StdDuration};

use axum::{
    Json, Router,
    body::Body,
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{delete, get, patch, post},
};
use axum_extra::extract::cookie::CookieJar;
use chrono::Utc;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom},
};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::{
    AppState, auth,
    error::ApiError,
    models::{
        App, Artifact, PublicReleaseArtifact, Release, StorageCleanupJob, StorageMigration,
        StorageMigrationItem, StorageProfile, StorageProfileResponse, User,
    },
    storage::{self, DownloadSource},
};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/openapi.json", get(openapi))
        .route("/api/docs", get(docs))
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/me", get(me))
        .route("/api/auth/me/password", post(change_my_password))
        .route("/api/users", get(list_users).post(create_user))
        .route("/api/users/{id}", patch(update_user))
        .route("/api/apps", get(list_apps).post(create_app))
        .route(
            "/api/apps/{id}",
            get(get_app).patch(update_app).delete(delete_app),
        )
        .route("/api/apps/{id}/transfer", post(transfer_app))
        .route(
            "/api/apps/{app_id}/releases",
            get(list_releases).post(create_release),
        )
        .route(
            "/api/releases/{id}",
            get(get_release)
                .patch(update_release)
                .delete(delete_release),
        )
        .route("/api/releases/{id}/publish", post(publish_release))
        .route("/api/releases/{id}/withdraw", post(withdraw_release))
        .route("/api/releases/{id}/artifacts", post(upload_artifact))
        .route("/api/artifacts/{id}", delete(delete_artifact))
        .route(
            "/api/storage-profiles",
            get(list_storage_profiles).post(create_storage_profile),
        )
        .route("/api/storage-profiles/{id}", delete(delete_storage_profile))
        .route(
            "/api/storage-profiles/{id}/activate",
            post(activate_storage_profile),
        )
        .route(
            "/api/storage-profiles/{id}/test",
            post(test_storage_profile),
        )
        .route(
            "/api/storage-migrations",
            get(list_storage_migrations).post(create_storage_migration),
        )
        .route("/api/storage-migrations/{id}", get(get_storage_migration))
        .route(
            "/api/storage-migrations/{id}/cancel",
            post(cancel_storage_migration),
        )
        .route(
            "/api/storage-migrations/{id}/retry",
            post(retry_storage_migration),
        )
        .route("/api/public/apps/{app_id}/update", get(check_update))
        .route(
            "/api/public/artifacts/{id}/download",
            get(download_artifact),
        )
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

async fn openapi() -> impl IntoResponse {
    Json(serde_json::json!({
        "openapi": "3.0.3", "info": {"title": "Update Service API", "version": "v1"},
        "paths": {
            "/api/public/apps/{app_id}/update": {"get": {"summary": "Check an application update"}},
            "/api/public/artifacts/{id}/download": {"get": {"summary": "Download a published artifact"}},
            "/api/auth/login": {"post": {"summary": "Create a management session"}},
            "/api/apps": {"get": {"summary": "List applications"}, "post": {"summary": "Create an application"}},
            "/api/storage-migrations": {
                "get": {"summary": "List storage migrations"},
                "post": {"summary": "Start a storage migration"}
            },
            "/api/storage-migrations/{id}": {"get": {"summary": "Get storage migration progress"}},
            "/api/storage-migrations/{id}/cancel": {"post": {"summary": "Cancel a storage migration"}},
            "/api/storage-migrations/{id}/retry": {"post": {"summary": "Retry unfinished migration items"}}
        }
    }))
}

async fn docs() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        "<!doctype html><title>Update Service API</title><h1>Update Service API</h1><p>Machine-readable schema: <a href='/api/openapi.json'>/api/openapi.json</a></p>",
    )
}

#[derive(Deserialize)]
struct LoginInput {
    username: String,
    password: String,
}
#[derive(Serialize)]
struct AuthResponse {
    user: PublicUser,
    csrf_token: String,
}

async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(input): Json<LoginInput>,
) -> Result<(CookieJar, Json<AuthResponse>), ApiError> {
    if input.username.trim().is_empty() || input.password.is_empty() {
        return Err(ApiError::validation("Username and password are required"));
    }
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE username=$1")
        .bind(input.username.trim())
        .fetch_optional(&state.db)
        .await?
        .filter(|user| user.enabled && auth::verify_password(&user.password_hash, &input.password))
        .ok_or_else(|| ApiError::Unauthorized("Invalid username or password".into()))?;
    let (token, csrf_token) = auth::create_session(&state, user.id).await?;
    Ok((
        jar.add(auth::session_cookie(&state, &token)),
        Json(AuthResponse {
            user: user.into(),
            csrf_token,
        }),
    ))
}

async fn logout(State(state): State<AppState>, jar: CookieJar) -> Result<CookieJar, ApiError> {
    if let Some(cookie) = jar.get(auth::SESSION_COOKIE) {
        let hash = sha256_hex(cookie.value().as_bytes());
        sqlx::query("DELETE FROM sessions WHERE token_hash=$1")
            .bind(hash)
            .execute(&state.db)
            .await?;
    }
    Ok(jar.remove(auth::clear_session_cookie()))
}

async fn me(State(state): State<AppState>, jar: CookieJar) -> Result<Json<AuthResponse>, ApiError> {
    let (user, csrf_token) = auth::current_user(&state, &jar).await?;
    Ok(Json(AuthResponse {
        user: user.into(),
        csrf_token,
    }))
}

#[derive(Deserialize)]
struct PasswordInput {
    current_password: String,
    new_password: String,
}
async fn change_my_password(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Json(input): Json<PasswordInput>,
) -> Result<StatusCode, ApiError> {
    let (user, csrf) = authenticated(&state, &jar, &headers).await?;
    validate_password(&input.new_password)?;
    if !auth::verify_password(&user.password_hash, &input.current_password) {
        return Err(ApiError::Forbidden("Current password is incorrect".into()));
    }
    let hash = auth::password_hash(&input.new_password)?;
    sqlx::query("UPDATE users SET password_hash=$1, updated_at=NOW() WHERE id=$2")
        .bind(hash)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE user_id=$1 AND csrf_token<>$2")
        .bind(user.id)
        .bind(csrf)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct PublicUser {
    id: Uuid,
    username: String,
    role: String,
    enabled: bool,
    created_at: chrono::DateTime<Utc>,
}
impl From<User> for PublicUser {
    fn from(v: User) -> Self {
        Self {
            id: v.id,
            username: v.username,
            role: v.role,
            enabled: v.enabled,
            created_at: v.created_at,
        }
    }
}

async fn list_users(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<Page<PublicUser>>, ApiError> {
    let (user, _) = auth::current_user(&state, &jar).await?;
    require_admin(&user)?;
    let users = sqlx::query_as::<_, User>("SELECT * FROM users ORDER BY created_at DESC")
        .fetch_all(&state.db)
        .await?;
    Ok(Json(Page::all(users.into_iter().map(Into::into).collect())))
}

#[derive(Deserialize)]
struct CreateUser {
    username: String,
    password: String,
    role: String,
}
async fn create_user(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Json(input): Json<CreateUser>,
) -> Result<(StatusCode, Json<PublicUser>), ApiError> {
    let (actor, _) = authenticated(&state, &jar, &headers).await?;
    require_admin(&actor)?;
    validate_username(&input.username)?;
    validate_password(&input.password)?;
    validate_role(&input.role)?;
    let user = sqlx::query_as::<_, User>(
        "INSERT INTO users (id,username,password_hash,role) VALUES ($1,$2,$3,$4) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(input.username.trim())
    .bind(auth::password_hash(&input.password)?)
    .bind(&input.role)
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::CREATED, Json(user.into())))
}

#[derive(Deserialize)]
struct UpdateUser {
    role: Option<String>,
    enabled: Option<bool>,
    password: Option<String>,
}
async fn update_user(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<UpdateUser>,
) -> Result<Json<PublicUser>, ApiError> {
    let (actor, _) = authenticated(&state, &jar, &headers).await?;
    require_admin(&actor)?;
    let target = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id=$1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("User not found"))?;
    if input.role.is_none() && input.enabled.is_none() && input.password.is_none() {
        return Err(ApiError::validation("No user fields supplied"));
    }
    if let Some(role) = &input.role {
        validate_role(role)?;
    }
    if let Some(password) = &input.password {
        validate_password(password)?;
    }
    if target.id == actor.id && input.enabled == Some(false) {
        return Err(ApiError::validation("You cannot disable your own account"));
    }
    let password_changed = input.password.is_some();
    let password_hash = match input.password {
        Some(v) => Some(auth::password_hash(&v)?),
        None => None,
    };
    let user = sqlx::query_as::<_, User>("UPDATE users SET role=COALESCE($1,role), enabled=COALESCE($2,enabled), password_hash=COALESCE($3,password_hash), updated_at=NOW() WHERE id=$4 RETURNING *")
        .bind(input.role).bind(input.enabled).bind(password_hash).bind(id).fetch_one(&state.db).await?;
    if !user.enabled || password_changed {
        sqlx::query("DELETE FROM sessions WHERE user_id=$1")
            .bind(id)
            .execute(&state.db)
            .await?;
    }
    Ok(Json(user.into()))
}

#[derive(Deserialize)]
struct AppQuery {
    page: Option<i64>,
    page_size: Option<i64>,
    name: Option<String>,
    status: Option<String>,
    owner_id: Option<Uuid>,
}
#[derive(Serialize)]
struct Page<T> {
    items: Vec<T>,
    total: i64,
    page: i64,
    page_size: i64,
}
impl<T> Page<T> {
    fn all(items: Vec<T>) -> Self {
        let total = items.len() as i64;
        Self {
            items,
            total,
            page: 1,
            page_size: total.max(1),
        }
    }
}

async fn list_apps(
    State(state): State<AppState>,
    jar: CookieJar,
    Query(query): Query<AppQuery>,
) -> Result<Json<Page<App>>, ApiError> {
    let (user, _) = auth::current_user(&state, &jar).await?;
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * page_size;
    let status = query
        .status
        .clone()
        .filter(|v| v == "active" || v == "deleted");
    let name_filter = query
        .name
        .clone()
        .filter(|v| !v.trim().is_empty())
        .map(|v| format!("%{}%", v.trim()));
    let requested_owner = if user.role == "admin" {
        query.owner_id
    } else {
        Some(user.id)
    };
    let apps = sqlx::query_as::<_, App>("SELECT * FROM apps WHERE ($1::uuid IS NULL OR owner_id=$1) AND ($2::text IS NULL OR status=$2) AND ($3::text IS NULL OR name ILIKE $3) ORDER BY updated_at DESC LIMIT $4 OFFSET $5")
        .bind(requested_owner).bind(status.clone()).bind(name_filter.clone()).bind(page_size).bind(offset).fetch_all(&state.db).await?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM apps WHERE ($1::uuid IS NULL OR owner_id=$1) AND ($2::text IS NULL OR status=$2) AND ($3::text IS NULL OR name ILIKE $3)")
        .bind(requested_owner).bind(status).bind(name_filter).fetch_one(&state.db).await?;
    Ok(Json(Page {
        items: apps,
        total,
        page,
        page_size,
    }))
}

#[derive(Deserialize)]
struct AppInput {
    name: String,
    #[serde(default)]
    description: String,
}
async fn create_app(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Json(input): Json<AppInput>,
) -> Result<(StatusCode, Json<App>), ApiError> {
    let (user, _) = authenticated(&state, &jar, &headers).await?;
    validate_app_input(&input)?;
    let app = sqlx::query_as::<_, App>(
        "INSERT INTO apps (id,name,description,owner_id) VALUES ($1,$2,$3,$4) RETURNING *",
    )
    .bind(Uuid::new_v4())
    .bind(input.name.trim())
    .bind(input.description.trim())
    .bind(user.id)
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::CREATED, Json(app)))
}

async fn get_app(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(id): Path<Uuid>,
) -> Result<Json<App>, ApiError> {
    let (user, _) = auth::current_user(&state, &jar).await?;
    Ok(Json(app_for_actor(&state, id, &user).await?))
}

async fn update_app(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<AppInput>,
) -> Result<Json<App>, ApiError> {
    let (user, _) = authenticated(&state, &jar, &headers).await?;
    app_for_actor(&state, id, &user).await?;
    validate_app_input(&input)?;
    let app = sqlx::query_as::<_, App>(
        "UPDATE apps SET name=$1,description=$2,updated_at=NOW() WHERE id=$3 RETURNING *",
    )
    .bind(input.name.trim())
    .bind(input.description.trim())
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(app))
}

async fn delete_app(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (user, _) = authenticated(&state, &jar, &headers).await?;
    app_for_actor(&state, id, &user).await?;
    let artifacts = sqlx::query_as::<_, Artifact>(
        "SELECT a.* FROM artifacts a JOIN releases r ON r.id=a.release_id WHERE r.app_id=$1",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let mut tx = state.db.begin().await?;
    queue_artifact_cleanup(&mut tx, &artifacts).await?;
    sqlx::query("DELETE FROM releases WHERE app_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM apps WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct TransferInput {
    owner_id: Uuid,
}
async fn transfer_app(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<TransferInput>,
) -> Result<Json<App>, ApiError> {
    let (user, _) = authenticated(&state, &jar, &headers).await?;
    require_admin(&user)?;
    let enabled: Option<bool> = sqlx::query_scalar("SELECT enabled FROM users WHERE id=$1")
        .bind(input.owner_id)
        .fetch_optional(&state.db)
        .await?;
    if enabled != Some(true) {
        return Err(ApiError::validation("New owner must be an enabled user"));
    }
    let app = sqlx::query_as::<_, App>(
        "UPDATE apps SET owner_id=$1,updated_at=NOW() WHERE id=$2 RETURNING *",
    )
    .bind(input.owner_id)
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| ApiError::not_found("Application not found"))?;
    Ok(Json(app))
}

#[derive(Deserialize)]
struct ReleaseInput {
    version: String,
    channel: String,
    #[serde(default)]
    release_notes: String,
}
async fn list_releases(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(app_id): Path<Uuid>,
) -> Result<Json<Page<Release>>, ApiError> {
    let (user, _) = auth::current_user(&state, &jar).await?;
    app_for_actor(&state, app_id, &user).await?;
    let releases = sqlx::query_as::<_, Release>(
        "SELECT * FROM releases WHERE app_id=$1 ORDER BY created_at DESC",
    )
    .bind(app_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(Page::all(releases)))
}

async fn create_release(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(app_id): Path<Uuid>,
    Json(input): Json<ReleaseInput>,
) -> Result<(StatusCode, Json<Release>), ApiError> {
    let (user, _) = authenticated(&state, &jar, &headers).await?;
    let app = app_for_actor(&state, app_id, &user).await?;
    if app.status != "active" {
        return Err(ApiError::validation(
            "Cannot create releases for deleted applications",
        ));
    }
    validate_release_input(&input)?;
    let release = sqlx::query_as::<_, Release>("INSERT INTO releases (id,app_id,version,channel,release_notes) VALUES ($1,$2,$3,$4,$5) RETURNING *")
        .bind(Uuid::new_v4()).bind(app_id).bind(input.version.trim()).bind(&input.channel).bind(input.release_notes.trim()).fetch_one(&state.db).await?;
    Ok((StatusCode::CREATED, Json(release)))
}

async fn get_release(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(id): Path<Uuid>,
) -> Result<Json<ReleaseDetail>, ApiError> {
    let (user, _) = auth::current_user(&state, &jar).await?;
    let release = release_for_actor(&state, id, &user).await?;
    Ok(Json(release_detail(&state, release).await?))
}

async fn update_release(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<ReleaseInput>,
) -> Result<Json<Release>, ApiError> {
    let (user, _) = authenticated(&state, &jar, &headers).await?;
    let release = release_for_actor(&state, id, &user).await?;
    if release.status != "draft" {
        return Err(ApiError::validation("Only draft releases can be changed"));
    }
    validate_release_input(&input)?;
    let release = sqlx::query_as::<_, Release>("UPDATE releases SET version=$1,channel=$2,release_notes=$3,updated_at=NOW() WHERE id=$4 RETURNING *")
        .bind(input.version.trim()).bind(input.channel).bind(input.release_notes.trim()).bind(id).fetch_one(&state.db).await?;
    Ok(Json(release))
}

async fn publish_release(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Release>, ApiError> {
    let (user, _) = authenticated(&state, &jar, &headers).await?;
    let release = release_for_actor(&state, id, &user).await?;
    if release.status != "draft" {
        return Err(ApiError::validation(
            "Only a draft release can be published",
        ));
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM artifacts WHERE release_id=$1")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    if count == 0 {
        return Err(ApiError::validation(
            "A release needs at least one artifact",
        ));
    }
    let release = sqlx::query_as::<_, Release>("UPDATE releases SET status='published',published_at=NOW(),updated_at=NOW() WHERE id=$1 RETURNING *").bind(id).fetch_one(&state.db).await?;
    Ok(Json(release))
}

async fn withdraw_release(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Release>, ApiError> {
    let (user, _) = authenticated(&state, &jar, &headers).await?;
    let release = release_for_actor(&state, id, &user).await?;
    if release.status != "published" {
        return Err(ApiError::validation(
            "Only a published release can be withdrawn",
        ));
    }
    let release = sqlx::query_as::<_, Release>(
        "UPDATE releases SET status='withdrawn',updated_at=NOW() WHERE id=$1 RETURNING *",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(release))
}

async fn delete_release(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (user, _) = authenticated(&state, &jar, &headers).await?;
    let release = release_for_actor(&state, id, &user).await?;
    if release.status != "draft" {
        return Err(ApiError::validation("Only draft releases can be deleted"));
    }
    let artifacts = sqlx::query_as::<_, Artifact>("SELECT * FROM artifacts WHERE release_id=$1")
        .bind(id)
        .fetch_all(&state.db)
        .await?;
    let mut tx = state.db.begin().await?;
    queue_artifact_cleanup(&mut tx, &artifacts).await?;
    sqlx::query("DELETE FROM releases WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct ReleaseDetail {
    #[serde(flatten)]
    release: Release,
    artifacts: Vec<Artifact>,
}
async fn release_detail(state: &AppState, release: Release) -> Result<ReleaseDetail, ApiError> {
    let artifacts = sqlx::query_as::<_, Artifact>(
        "SELECT * FROM artifacts WHERE release_id=$1 ORDER BY platform",
    )
    .bind(release.id)
    .fetch_all(&state.db)
    .await?;
    Ok(ReleaseDetail { release, artifacts })
}

async fn upload_artifact(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    multipart: Multipart,
) -> Result<(StatusCode, Json<Artifact>), ApiError> {
    let (user, _) = authenticated(&state, &jar, &headers).await?;
    let release = release_for_actor(&state, id, &user).await?;
    if release.status != "draft" {
        return Err(ApiError::validation(
            "Artifacts can only be changed on draft releases",
        ));
    }
    let (platform, filename, content_type, temp_path, size_bytes, sha256) =
        save_multipart_to_temp(&state, multipart).await?;
    let profile = match sqlx::query_as::<_, StorageProfile>(
        "SELECT * FROM storage_profiles WHERE is_active=true",
    )
    .fetch_optional(&state.db)
    .await?
    {
        Some(profile) => profile,
        None => {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(ApiError::validation("No active storage profile configured"));
        }
    };
    let artifact_id = Uuid::new_v4();
    let object_key = format!(
        "artifacts/{}/{}/{}",
        release.app_id, release.id, artifact_id
    );
    let uploaded = storage::upload_temp_file_with_metadata(
        &state,
        &profile,
        &object_key,
        &temp_path,
        Some(&content_type),
        Some(&sha256),
    )
    .await;
    if let Err(error) = uploaded {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(error);
    }

    // A migration or an explicit profile activation can cut over while the
    // multipart upload is in flight. Finalize under the same advisory lock as
    // the cutover and, if needed, relay the still-local temp file to the new
    // active profile before creating the artifact row.
    let mut profile = profile;
    let artifact_result: Result<Artifact, sqlx::Error> = loop {
        let mut tx = state.db.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(storage::STORAGE_MUTATION_LOCK_KEY)
            .execute(&mut *tx)
            .await?;
        let active = sqlx::query_as::<_, StorageProfile>(
            "SELECT * FROM storage_profiles WHERE is_active=true",
        )
        .fetch_optional(&mut *tx)
        .await?;
        let Some(active) = active else {
            tx.rollback().await?;
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(ApiError::validation("No active storage profile configured"));
        };
        if active.id != profile.id {
            tx.rollback().await?;
            if let Err(cleanup) = storage::delete_object(&state, &profile, &object_key).await {
                tracing::warn!(error=%cleanup, object_key, "failed to clean up pre-cutover upload");
                let _ = sqlx::query("INSERT INTO storage_cleanup_jobs (id,storage_profile_id,object_key,last_error) VALUES ($1,$2,$3,$4)")
                    .bind(Uuid::new_v4())
                    .bind(profile.id)
                    .bind(&object_key)
                    .bind(cleanup.to_string())
                    .execute(&state.db)
                    .await;
            }
            let uploaded = storage::upload_temp_file_with_metadata(
                &state,
                &active,
                &object_key,
                &temp_path,
                Some(&content_type),
                Some(&sha256),
            )
            .await;
            if let Err(error) = uploaded {
                let _ = tokio::fs::remove_file(&temp_path).await;
                return Err(error);
            }
            profile = active;
            continue;
        }
        let artifact = sqlx::query_as::<_, Artifact>("INSERT INTO artifacts (id,release_id,platform,original_file_name,content_type,size_bytes,sha256,storage_profile_id,object_key) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING *")
            .bind(artifact_id)
            .bind(id)
            .bind(&platform)
            .bind(&filename)
            .bind(&content_type)
            .bind(size_bytes as i64)
            .bind(&sha256)
            .bind(profile.id)
            .bind(&object_key)
            .fetch_one(&mut *tx)
            .await;
        match artifact {
            Ok(value) => {
                tx.commit().await?;
                break Ok(value);
            }
            Err(error) => {
                tx.rollback().await?;
                break Err(error);
            }
        }
    };
    let _ = tokio::fs::remove_file(&temp_path).await;
    match artifact_result {
        Ok(value) => Ok((StatusCode::CREATED, Json(value))),
        Err(error) => {
            if let Err(cleanup) = storage::delete_object(&state, &profile, &object_key).await {
                tracing::error!(error=%cleanup, object_key, "failed to clean up orphan artifact; queuing retry");
                sqlx::query("INSERT INTO storage_cleanup_jobs (id,storage_profile_id,object_key,last_error) VALUES ($1,$2,$3,$4)")
                    .bind(Uuid::new_v4()).bind(profile.id).bind(&object_key).bind(cleanup.to_string()).execute(&state.db).await?;
            }
            Err(error.into())
        }
    }
}

async fn delete_artifact(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (user, _) = authenticated(&state, &jar, &headers).await?;
    let artifact = sqlx::query_as::<_, Artifact>("SELECT a.* FROM artifacts a JOIN releases r ON r.id=a.release_id JOIN apps app ON app.id=r.app_id WHERE a.id=$1 AND ($2='admin' OR app.owner_id=$3)")
        .bind(id).bind(&user.role).bind(user.id).fetch_optional(&state.db).await?.ok_or_else(|| ApiError::not_found("Artifact not found"))?;
    let release_status: String = sqlx::query_scalar("SELECT status FROM releases WHERE id=$1")
        .bind(artifact.release_id)
        .fetch_one(&state.db)
        .await?;
    if release_status != "draft" {
        return Err(ApiError::validation(
            "Artifacts can only be deleted from draft releases",
        ));
    }
    let mut tx = state.db.begin().await?;
    queue_artifact_cleanup(&mut tx, std::slice::from_ref(&artifact)).await?;
    sqlx::query("DELETE FROM artifacts WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct CreateStorageProfile {
    name: String,
    backend: String,
    config: serde_json::Value,
    secret: Option<String>,
}
async fn list_storage_profiles(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<Page<StorageProfileResponse>>, ApiError> {
    let (user, _) = auth::current_user(&state, &jar).await?;
    require_admin(&user)?;
    let profiles = sqlx::query_as::<_, StorageProfile>(
        "SELECT * FROM storage_profiles ORDER BY created_at DESC",
    )
    .fetch_all(&state.db)
    .await?;
    let mut responses = Vec::with_capacity(profiles.len());
    for profile in profiles {
        let (artifact_count, artifact_bytes): (i64, i64) = sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(size_bytes),0)::bigint FROM artifacts WHERE storage_profile_id=$1",
        )
        .bind(profile.id)
        .fetch_one(&state.db)
        .await?;
        let mut response: StorageProfileResponse = profile.into();
        response.artifact_count = artifact_count;
        response.artifact_bytes = artifact_bytes;
        responses.push(response);
    }
    Ok(Json(Page::all(responses)))
}

async fn create_storage_profile(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Json(input): Json<CreateStorageProfile>,
) -> Result<(StatusCode, Json<StorageProfileResponse>), ApiError> {
    let (user, _) = authenticated(&state, &jar, &headers).await?;
    require_admin(&user)?;
    if input.name.trim().is_empty() || input.name.len() > 100 {
        return Err(ApiError::validation(
            "Storage profile name must be 1-100 characters",
        ));
    }
    storage::validate_profile(&input.backend, &input.config, input.secret.as_deref())?;
    let encrypted = input
        .secret
        .map(|s| storage::encrypt_secret(&state.config.settings_master_key, &s))
        .transpose()?;
    let profile = sqlx::query_as::<_, StorageProfile>("INSERT INTO storage_profiles (id,name,backend,config,secret_encrypted,created_by) VALUES ($1,$2,$3,$4,$5,$6) RETURNING *")
        .bind(Uuid::new_v4()).bind(input.name.trim()).bind(&input.backend).bind(input.config).bind(encrypted).bind(user.id).fetch_one(&state.db).await?;
    Ok((StatusCode::CREATED, Json(profile.into())))
}

async fn activate_storage_profile(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<StorageProfileResponse>, ApiError> {
    let (user, _) = authenticated(&state, &jar, &headers).await?;
    require_admin(&user)?;
    let profile = sqlx::query_as::<_, StorageProfile>("SELECT * FROM storage_profiles WHERE id=$1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("Storage profile not found"))?;
    storage::test_profile(&state, &profile).await?;
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(storage::STORAGE_MUTATION_LOCK_KEY)
        .execute(&mut *tx)
        .await?;
    let active_migration: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM storage_migrations WHERE status IN ('queued','running') LIMIT 1",
    )
    .fetch_optional(&mut *tx)
    .await?;
    if active_migration.is_some() {
        return Err(ApiError::Conflict(
            "The active storage profile cannot be changed during a migration".into(),
        ));
    }
    sqlx::query("UPDATE storage_profiles SET is_active=false WHERE is_active=true")
        .execute(&mut *tx)
        .await?;
    let updated = sqlx::query_as::<_, StorageProfile>(
        "UPDATE storage_profiles SET is_active=true WHERE id=$1 RETURNING *",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(updated.into()))
}

async fn test_storage_profile(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (user, _) = authenticated(&state, &jar, &headers).await?;
    require_admin(&user)?;
    let profile = sqlx::query_as::<_, StorageProfile>("SELECT * FROM storage_profiles WHERE id=$1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("Storage profile not found"))?;
    storage::test_profile(&state, &profile).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_storage_profile(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let (user, _) = authenticated(&state, &jar, &headers).await?;
    require_admin(&user)?;
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(storage::STORAGE_MUTATION_LOCK_KEY)
        .execute(&mut *tx)
        .await?;
    let profile = sqlx::query_as::<_, StorageProfile>(
        "SELECT * FROM storage_profiles WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Storage profile not found"))?;
    if profile.is_active {
        return Err(ApiError::validation(
            "The active storage profile cannot be deleted",
        ));
    }
    let references: i64 = sqlx::query_scalar("SELECT (SELECT COUNT(*) FROM artifacts WHERE storage_profile_id=$1) + (SELECT COUNT(*) FROM storage_cleanup_jobs WHERE storage_profile_id=$1) + (SELECT COUNT(*) FROM storage_migrations WHERE status IN ('queued','running') AND (source_profile_id=$1 OR destination_profile_id=$1))")
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
    if references > 0 {
        return Err(ApiError::Conflict(
            "Storage profile is still referenced by artifacts, cleanup jobs, or an active migration".into(),
        ));
    }
    sqlx::query("DELETE FROM storage_profiles WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct CreateStorageMigration {
    source_profile_id: Uuid,
    destination_profile_id: Uuid,
}

#[derive(Debug, Serialize)]
struct StorageMigrationDetailResponse {
    #[serde(flatten)]
    migration: StorageMigration,
    failed_items: Vec<StorageMigrationItem>,
}

async fn create_storage_migration(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Json(input): Json<CreateStorageMigration>,
) -> Result<(StatusCode, Json<StorageMigration>), ApiError> {
    let (user, _) = authenticated(&state, &jar, &headers).await?;
    require_admin(&user)?;
    if input.source_profile_id == input.destination_profile_id {
        return Err(ApiError::validation(
            "Source and destination storage profiles must differ",
        ));
    }

    let source = sqlx::query_as::<_, StorageProfile>("SELECT * FROM storage_profiles WHERE id=$1")
        .bind(input.source_profile_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("Source storage profile not found"))?;
    let destination =
        sqlx::query_as::<_, StorageProfile>("SELECT * FROM storage_profiles WHERE id=$1")
            .bind(input.destination_profile_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| ApiError::not_found("Destination storage profile not found"))?;
    validate_migration_direction(&source.backend, &destination.backend)?;
    storage::test_profile(&state, &source).await?;
    storage::test_profile(&state, &destination).await?;

    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(storage::STORAGE_MUTATION_LOCK_KEY)
        .execute(&mut *tx)
        .await?;
    let active_job: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM storage_migrations WHERE status IN ('queued','running') LIMIT 1 FOR UPDATE",
    )
    .fetch_optional(&mut *tx)
    .await?;
    if active_job.is_some() {
        return Err(ApiError::Conflict(
            "Another storage migration is already running".into(),
        ));
    }

    // Re-read both rows while holding the mutation lock so a profile cannot be
    // deleted or changed between validation and the cutover.
    let source = sqlx::query_as::<_, StorageProfile>(
        "SELECT * FROM storage_profiles WHERE id=$1 FOR UPDATE",
    )
    .bind(input.source_profile_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Source storage profile not found"))?;
    let destination = sqlx::query_as::<_, StorageProfile>(
        "SELECT * FROM storage_profiles WHERE id=$1 FOR UPDATE",
    )
    .bind(input.destination_profile_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Destination storage profile not found"))?;
    validate_migration_direction(&source.backend, &destination.backend)?;

    // The destination becomes the upload target atomically with task creation.
    sqlx::query("UPDATE storage_profiles SET is_active=false WHERE is_active=true")
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE storage_profiles SET is_active=true WHERE id=$1")
        .bind(destination.id)
        .execute(&mut *tx)
        .await?;

    let artifacts = sqlx::query_as::<_, Artifact>(
        "SELECT * FROM artifacts WHERE storage_profile_id=$1 ORDER BY created_at, id",
    )
    .bind(source.id)
    .fetch_all(&mut *tx)
    .await?;
    let total_bytes = artifacts
        .iter()
        .map(|artifact| artifact.size_bytes.max(0))
        .fold(0_i64, |total, value| total.saturating_add(value));
    let migration_id = Uuid::new_v4();
    sqlx::query("INSERT INTO storage_migrations (id,source_profile_id,destination_profile_id,source_profile_name,destination_profile_name,source_backend,destination_backend,total_objects,total_bytes,requested_by) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
        .bind(migration_id)
        .bind(source.id)
        .bind(destination.id)
        .bind(&source.name)
        .bind(&destination.name)
        .bind(&source.backend)
        .bind(&destination.backend)
        .bind(artifacts.len() as i64)
        .bind(total_bytes)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    for artifact in artifacts {
        sqlx::query("INSERT INTO storage_migration_items (id,migration_id,artifact_id,artifact_id_snapshot,object_key,size_bytes,sha256,content_type) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(Uuid::new_v4())
            .bind(migration_id)
            .bind(artifact.id)
            .bind(artifact.id)
            .bind(artifact.object_key)
            .bind(artifact.size_bytes.max(0))
            .bind(artifact.sha256)
            .bind(artifact.content_type)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    let migration =
        sqlx::query_as::<_, StorageMigration>("SELECT * FROM storage_migrations WHERE id=$1")
            .bind(migration_id)
            .fetch_one(&state.db)
            .await?;
    Ok((StatusCode::ACCEPTED, Json(migration)))
}

async fn list_storage_migrations(
    State(state): State<AppState>,
    jar: CookieJar,
) -> Result<Json<Page<StorageMigration>>, ApiError> {
    let (user, _) = auth::current_user(&state, &jar).await?;
    require_admin(&user)?;
    let migrations = sqlx::query_as::<_, StorageMigration>(
        "SELECT * FROM storage_migrations WHERE created_at >= NOW() - INTERVAL '30 days' ORDER BY created_at DESC LIMIT 100",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(Json(Page::all(migrations)))
}

async fn get_storage_migration(
    State(state): State<AppState>,
    jar: CookieJar,
    Path(id): Path<Uuid>,
) -> Result<Json<StorageMigrationDetailResponse>, ApiError> {
    let (user, _) = auth::current_user(&state, &jar).await?;
    require_admin(&user)?;
    let migration =
        sqlx::query_as::<_, StorageMigration>("SELECT * FROM storage_migrations WHERE id=$1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| ApiError::not_found("Storage migration not found"))?;
    let failed_items = sqlx::query_as::<_, StorageMigrationItem>(
        "SELECT * FROM storage_migration_items WHERE migration_id=$1 AND status IN ('failed','pending','running') ORDER BY created_at LIMIT 200",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(StorageMigrationDetailResponse {
        migration,
        failed_items,
    }))
}

async fn cancel_storage_migration(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<StorageMigration>, ApiError> {
    let (user, _) = authenticated(&state, &jar, &headers).await?;
    require_admin(&user)?;
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(storage::STORAGE_MUTATION_LOCK_KEY)
        .execute(&mut *tx)
        .await?;
    let migration = sqlx::query_as::<_, StorageMigration>(
        "SELECT * FROM storage_migrations WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Storage migration not found"))?;
    match migration.status.as_str() {
        "queued" => {
            sqlx::query("UPDATE storage_migrations SET status='cancelled',cancel_requested=true,finished_at=NOW(),updated_at=NOW() WHERE id=$1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        "running" => {
            sqlx::query(
                "UPDATE storage_migrations SET cancel_requested=true,updated_at=NOW() WHERE id=$1",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        _ => {
            return Err(ApiError::validation(
                "Only queued or running migrations can be cancelled",
            ));
        }
    }
    tx.commit().await?;
    let updated =
        sqlx::query_as::<_, StorageMigration>("SELECT * FROM storage_migrations WHERE id=$1")
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    Ok(Json(updated))
}

async fn retry_storage_migration(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<StorageMigration>, ApiError> {
    let (user, _) = authenticated(&state, &jar, &headers).await?;
    require_admin(&user)?;
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(storage::STORAGE_MUTATION_LOCK_KEY)
        .execute(&mut *tx)
        .await?;
    let migration = sqlx::query_as::<_, StorageMigration>(
        "SELECT * FROM storage_migrations WHERE id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::not_found("Storage migration not found"))?;
    if matches!(
        migration.status.as_str(),
        "queued" | "running" | "completed"
    ) {
        return Err(ApiError::validation(
            "Only failed or cancelled migrations can be retried",
        ));
    }
    let active_job: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM storage_migrations WHERE status IN ('queued','running') AND id<>$1 LIMIT 1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    if active_job.is_some() {
        return Err(ApiError::Conflict(
            "Another storage migration is already running".into(),
        ));
    }
    let reset_pending = migration.status == "cancelled";
    if reset_pending {
        sqlx::query("UPDATE storage_migration_items SET status='pending',attempts=0,next_attempt_at=NOW(),last_error=NULL,finished_at=NULL,updated_at=NOW() WHERE migration_id=$1 AND status IN ('failed','pending','running')")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    } else {
        sqlx::query("UPDATE storage_migration_items SET status='pending',attempts=0,next_attempt_at=NOW(),last_error=NULL,finished_at=NULL,updated_at=NOW() WHERE migration_id=$1 AND status='failed'")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("UPDATE storage_migrations SET status='queued',cancel_requested=false,failed_objects=0,last_error=NULL,started_at=NULL,finished_at=NULL,updated_at=NOW() WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let updated =
        sqlx::query_as::<_, StorageMigration>("SELECT * FROM storage_migrations WHERE id=$1")
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    Ok(Json(updated))
}

const MIGRATION_MAX_ATTEMPTS: i32 = 5;
const MIGRATION_WORKER_LOCK_KEY: i64 = 4_827_193_605;

/// Reset in-flight rows after an ungraceful API process restart. The worker is
/// intentionally durable: a process crash must not strand a task in `running`.
pub async fn recover_migration_jobs(state: &AppState) -> Result<(), ApiError> {
    let mut connection = state.db.acquire().await?;
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(MIGRATION_WORKER_LOCK_KEY)
        .fetch_one(&mut *connection)
        .await?;
    if !acquired {
        return Ok(());
    }
    let result = async {
        let mut tx = state.db.begin().await?;
        sqlx::query(
            "UPDATE storage_migrations SET status='queued',updated_at=NOW() WHERE status='running'",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE storage_migration_items SET status='pending',next_attempt_at=NOW(),updated_at=NOW() WHERE status='running'")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    .await;
    let unlock = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(MIGRATION_WORKER_LOCK_KEY)
        .execute(&mut *connection)
        .await;
    if let Err(error) = unlock {
        if result.is_ok() {
            return Err(error.into());
        }
        tracing::warn!(%error, "failed to release storage migration recovery lock");
    }
    result
}

/// Claim and run at most one queued migration. The caller invokes this from a
/// dedicated loop, so a large migration never blocks cleanup requests.
pub async fn process_migration_jobs(state: &AppState) -> Result<(), ApiError> {
    let mut connection = state.db.acquire().await?;
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(MIGRATION_WORKER_LOCK_KEY)
        .fetch_one(&mut *connection)
        .await?;
    if !acquired {
        return Ok(());
    }
    let result = process_migration_jobs_locked(state).await;
    let unlock = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(MIGRATION_WORKER_LOCK_KEY)
        .execute(&mut *connection)
        .await;
    if let Err(error) = unlock {
        if result.is_ok() {
            return Err(error.into());
        }
        tracing::warn!(%error, "failed to release storage migration worker lock");
    }
    result
}

async fn process_migration_jobs_locked(state: &AppState) -> Result<(), ApiError> {
    let running = sqlx::query_as::<_, StorageMigration>(
        "SELECT * FROM storage_migrations WHERE status='running' ORDER BY created_at LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await?;
    let migration = if let Some(migration) = running {
        // A prior worker pass returned on a transient database error. No other
        // worker can be active while the session lock is held, so it is safe
        // to put the interrupted item back in the durable pending queue.
        sqlx::query("UPDATE storage_migration_items SET status='pending',next_attempt_at=NOW(),updated_at=NOW() WHERE migration_id=$1 AND status='running'")
            .bind(migration.id)
            .execute(&state.db)
            .await?;
        migration
    } else {
        let Some(migration) = claim_next_migration(state).await? else {
            return Ok(());
        };
        migration
    };
    process_migration(state, migration.id).await
}

async fn claim_next_migration(state: &AppState) -> Result<Option<StorageMigration>, ApiError> {
    let mut tx = state.db.begin().await?;
    let migration = sqlx::query_as::<_, StorageMigration>(
        "SELECT * FROM storage_migrations WHERE status='queued' ORDER BY created_at LIMIT 1 FOR UPDATE SKIP LOCKED",
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(migration) = migration else {
        tx.commit().await?;
        return Ok(None);
    };
    sqlx::query("UPDATE storage_migrations SET status='running',started_at=COALESCE(started_at,NOW()),updated_at=NOW() WHERE id=$1")
        .bind(migration.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(StorageMigration {
        status: "running".into(),
        started_at: Some(Utc::now()),
        ..migration
    }))
}

async fn process_migration(state: &AppState, migration_id: Uuid) -> Result<(), ApiError> {
    let mut empty_scans = 0u8;
    loop {
        let migration = load_migration(state, migration_id).await?;
        if migration.status != "running" {
            return Ok(());
        }
        if migration.cancel_requested {
            finalize_migration(state, migration_id, "cancelled").await?;
            return Ok(());
        }

        if let Some(item) = claim_next_migration_item(state, migration_id).await? {
            empty_scans = 0;
            process_migration_item(state, &migration, &item).await?;
            continue;
        }

        // New artifacts can finish an upload while the cutover transaction is
        // running. Add any source references not present in the initial
        // snapshot before deciding that the task is complete.
        let added = enqueue_missing_migration_items(state, &migration).await?;
        if added > 0 {
            empty_scans = 0;
            continue;
        }

        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM storage_migration_items WHERE migration_id=$1 AND status='pending'",
        )
        .bind(migration_id)
        .fetch_one(&state.db)
        .await?;
        if pending > 0 {
            tokio::time::sleep(StdDuration::from_secs(1)).await;
            continue;
        }

        // Require two empty source scans separated by a short delay. This is
        // a final quiescence window for an upload that started before cutover.
        if empty_scans == 0 {
            empty_scans = 1;
            tokio::time::sleep(StdDuration::from_secs(1)).await;
            continue;
        }
        let added = enqueue_missing_migration_items(state, &migration).await?;
        if added > 0 {
            empty_scans = 0;
            continue;
        }

        let failed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM storage_migration_items WHERE migration_id=$1 AND status='failed'",
        )
        .bind(migration_id)
        .fetch_one(&state.db)
        .await?;
        finalize_migration(
            state,
            migration_id,
            if failed > 0 {
                "partial_failed"
            } else {
                "completed"
            },
        )
        .await?;
        return Ok(());
    }
}

async fn load_migration(state: &AppState, id: Uuid) -> Result<StorageMigration, ApiError> {
    sqlx::query_as::<_, StorageMigration>("SELECT * FROM storage_migrations WHERE id=$1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("Storage migration not found"))
}

async fn claim_next_migration_item(
    state: &AppState,
    migration_id: Uuid,
) -> Result<Option<StorageMigrationItem>, ApiError> {
    let mut tx = state.db.begin().await?;
    let item = sqlx::query_as::<_, StorageMigrationItem>(
        "SELECT * FROM storage_migration_items WHERE migration_id=$1 AND status='pending' AND next_attempt_at <= NOW() ORDER BY created_at LIMIT 1 FOR UPDATE SKIP LOCKED",
    )
    .bind(migration_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(item) = item else {
        tx.commit().await?;
        return Ok(None);
    };
    sqlx::query("UPDATE storage_migration_items SET status='running',started_at=COALESCE(started_at,NOW()),updated_at=NOW() WHERE id=$1")
        .bind(item.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(StorageMigrationItem {
        status: "running".into(),
        started_at: Some(Utc::now()),
        ..item
    }))
}

async fn enqueue_missing_migration_items(
    state: &AppState,
    migration: &StorageMigration,
) -> Result<u64, ApiError> {
    let Some(source_id) = migration.source_profile_id else {
        return Ok(0);
    };
    let artifacts = sqlx::query_as::<_, Artifact>(
        "SELECT * FROM artifacts WHERE storage_profile_id=$1 ORDER BY created_at, id",
    )
    .bind(source_id)
    .fetch_all(&state.db)
    .await?;
    if artifacts.is_empty() {
        return Ok(0);
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(storage::STORAGE_MUTATION_LOCK_KEY)
        .execute(&mut *tx)
        .await?;
    let mut added = 0u64;
    let mut added_bytes = 0i64;
    for artifact in artifacts {
        let result = sqlx::query("INSERT INTO storage_migration_items (id,migration_id,artifact_id,artifact_id_snapshot,object_key,size_bytes,sha256,content_type) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT (migration_id,artifact_id_snapshot) DO NOTHING")
            .bind(Uuid::new_v4())
            .bind(migration.id)
            .bind(artifact.id)
            .bind(artifact.id)
            .bind(artifact.object_key)
            .bind(artifact.size_bytes.max(0))
            .bind(artifact.sha256)
            .bind(artifact.content_type)
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() > 0 {
            added += 1;
            added_bytes = added_bytes.saturating_add(artifact.size_bytes.max(0));
        }
    }
    if added > 0 {
        sqlx::query("UPDATE storage_migrations SET total_objects=total_objects+$1,total_bytes=total_bytes+$2,updated_at=NOW() WHERE id=$3")
            .bind(added as i64)
            .bind(added_bytes)
            .bind(migration.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(added)
}

async fn process_migration_item(
    state: &AppState,
    migration: &StorageMigration,
    item: &StorageMigrationItem,
) -> Result<(), ApiError> {
    let Some(source_id) = migration.source_profile_id else {
        return record_migration_failure(
            state,
            migration.id,
            item,
            "Migration source profile is no longer available",
        )
        .await;
    };
    let Some(destination_id) = migration.destination_profile_id else {
        return record_migration_failure(
            state,
            migration.id,
            item,
            "Migration destination profile is no longer available",
        )
        .await;
    };
    let artifact = sqlx::query_as::<_, Artifact>("SELECT * FROM artifacts WHERE id=$1")
        .bind(item.artifact_id_snapshot)
        .fetch_optional(&state.db)
        .await?;
    let Some(artifact) = artifact else {
        return finish_migration_item(state, migration.id, item, false, true, false).await;
    };

    if artifact.storage_profile_id == destination_id {
        return finish_migration_item(state, migration.id, item, true, false, false).await;
    }
    if artifact.storage_profile_id != source_id {
        // Another maintenance action moved the reference. Do not overwrite
        // that decision; the destination object is cleaned only when this
        // attempt created it.
        return finish_migration_item(state, migration.id, item, false, true, false).await;
    }

    let source = sqlx::query_as::<_, StorageProfile>("SELECT * FROM storage_profiles WHERE id=$1")
        .bind(source_id)
        .fetch_optional(&state.db)
        .await?;
    let destination =
        sqlx::query_as::<_, StorageProfile>("SELECT * FROM storage_profiles WHERE id=$1")
            .bind(destination_id)
            .fetch_optional(&state.db)
            .await?;
    let (Some(source), Some(destination)) = (source, destination) else {
        return record_migration_failure(
            state,
            migration.id,
            item,
            "Migration storage profile is no longer available",
        )
        .await;
    };

    match storage::migrate_object(
        state,
        &source,
        &destination,
        &item.object_key,
        item.size_bytes,
        &item.sha256,
        &item.content_type,
    )
    .await
    {
        Ok(outcome) => {
            let created_destination = matches!(outcome, storage::TransferOutcome::Copied);
            finish_migration_item(state, migration.id, item, true, false, created_destination).await
        }
        Err(error) => record_migration_failure(state, migration.id, item, &error.to_string()).await,
    }
}

async fn finish_migration_item(
    state: &AppState,
    migration_id: Uuid,
    item: &StorageMigrationItem,
    succeeded: bool,
    skipped: bool,
    created_destination: bool,
) -> Result<(), ApiError> {
    let Some(migration) =
        sqlx::query_as::<_, StorageMigration>("SELECT * FROM storage_migrations WHERE id=$1")
            .bind(migration_id)
            .fetch_optional(&state.db)
            .await?
    else {
        return Ok(());
    };
    let Some(destination_id) = migration.destination_profile_id else {
        return Ok(());
    };
    let Some(source_id) = migration.source_profile_id else {
        return Ok(());
    };

    let mut cleanup_destination = false;
    let mut final_skipped = skipped;
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(storage::STORAGE_MUTATION_LOCK_KEY)
        .execute(&mut *tx)
        .await?;
    if succeeded {
        let updated = sqlx::query(
            "UPDATE artifacts SET storage_profile_id=$1 WHERE id=$2 AND storage_profile_id=$3",
        )
        .bind(destination_id)
        .bind(item.artifact_id_snapshot)
        .bind(source_id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            let current: Option<Uuid> =
                sqlx::query_scalar("SELECT storage_profile_id FROM artifacts WHERE id=$1")
                    .bind(item.artifact_id_snapshot)
                    .fetch_optional(&mut *tx)
                    .await?;
            if current != Some(destination_id) {
                final_skipped = true;
                cleanup_destination = created_destination;
            }
        }
    }
    let terminal_status = if final_skipped {
        "skipped"
    } else {
        "succeeded"
    };
    let updated = sqlx::query("UPDATE storage_migration_items SET status=$1,bytes_copied=$2,finished_at=NOW(),updated_at=NOW() WHERE id=$3 AND status='running'")
        .bind(terminal_status)
        .bind(if final_skipped { 0_i64 } else { item.size_bytes.max(0) })
        .bind(item.id)
        .execute(&mut *tx)
        .await?;
    if updated.rows_affected() > 0 {
        sqlx::query("UPDATE storage_migrations SET completed_objects=completed_objects+1,completed_bytes=completed_bytes+$1,skipped_objects=skipped_objects+$2,updated_at=NOW() WHERE id=$3")
            .bind(if final_skipped { 0_i64 } else { item.size_bytes.max(0) })
            .bind(if final_skipped { 1_i64 } else { 0_i64 })
            .bind(migration_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    if cleanup_destination {
        if let Some(profile) =
            sqlx::query_as::<_, StorageProfile>("SELECT * FROM storage_profiles WHERE id=$1")
                .bind(destination_id)
                .fetch_optional(&state.db)
                .await?
        {
            if let Err(error) = storage::delete_object(state, &profile, &item.object_key).await {
                tracing::warn!(error = %error, migration_id = %migration_id, object_key = %item.object_key, "failed to clean up migration orphan");
                let _ = sqlx::query("INSERT INTO storage_cleanup_jobs (id,storage_profile_id,object_key,last_error) VALUES ($1,$2,$3,$4)")
                    .bind(Uuid::new_v4())
                    .bind(destination_id)
                    .bind(&item.object_key)
                    .bind(error.to_string())
                    .execute(&state.db)
                    .await;
            }
        }
    }
    Ok(())
}

async fn record_migration_failure(
    state: &AppState,
    migration_id: Uuid,
    item: &StorageMigrationItem,
    message: &str,
) -> Result<(), ApiError> {
    let attempts = item.attempts.saturating_add(1);
    let sanitized = message.chars().take(1000).collect::<String>();
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(storage::STORAGE_MUTATION_LOCK_KEY)
        .execute(&mut *tx)
        .await?;
    if attempts >= MIGRATION_MAX_ATTEMPTS {
        let updated = sqlx::query("UPDATE storage_migration_items SET status='failed',attempts=$1,last_error=$2,finished_at=NOW(),updated_at=NOW() WHERE id=$3 AND status='running'")
            .bind(attempts)
            .bind(&sanitized)
            .bind(item.id)
            .execute(&mut *tx)
            .await?;
        if updated.rows_affected() > 0 {
            sqlx::query("UPDATE storage_migrations SET failed_objects=failed_objects+1,last_error=$1,updated_at=NOW() WHERE id=$2")
                .bind(&sanitized)
                .bind(migration_id)
                .execute(&mut *tx)
                .await?;
        }
    } else {
        let delay = migration_retry_delay_seconds(attempts);
        sqlx::query("UPDATE storage_migration_items SET status='pending',attempts=$1,next_attempt_at=NOW() + ($2 * INTERVAL '1 second'),last_error=$3,updated_at=NOW() WHERE id=$4 AND status='running'")
            .bind(attempts)
            .bind(delay)
            .bind(&sanitized)
            .bind(item.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE storage_migrations SET last_error=$1,updated_at=NOW() WHERE id=$2")
            .bind(&sanitized)
            .bind(migration_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn finalize_migration(
    state: &AppState,
    migration_id: Uuid,
    status: &str,
) -> Result<(), ApiError> {
    sqlx::query("UPDATE storage_migrations SET status=$1,finished_at=NOW(),updated_at=NOW() WHERE id=$2 AND status='running'")
        .bind(status)
        .bind(migration_id)
        .execute(&state.db)
        .await?;
    Ok(())
}

pub async fn purge_old_migrations(state: &AppState) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM storage_migrations WHERE status IN ('completed','partial_failed','cancelled') AND finished_at < NOW() - INTERVAL '30 days'")
        .execute(&state.db)
        .await?;
    Ok(())
}

fn validate_migration_direction(source: &str, destination: &str) -> Result<(), ApiError> {
    if matches!(
        (source, destination),
        ("s3", "s3") | ("s3", "local") | ("local", "s3")
    ) {
        Ok(())
    } else {
        Err(ApiError::validation(
            "Storage migration supports S3 to S3, S3 to local, or local to S3",
        ))
    }
}

fn migration_retry_delay_seconds(attempts: i32) -> i64 {
    30_i64.saturating_mul(2_i64.pow((attempts - 1).clamp(0, 8) as u32))
}

async fn queue_artifact_cleanup(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    artifacts: &[Artifact],
) -> Result<(), ApiError> {
    for artifact in artifacts {
        sqlx::query(
            "INSERT INTO storage_cleanup_jobs (id,storage_profile_id,object_key) VALUES ($1,$2,$3)",
        )
        .bind(Uuid::new_v4())
        .bind(artifact.storage_profile_id)
        .bind(&artifact.object_key)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

pub async fn process_cleanup_jobs(state: &AppState) -> Result<(), ApiError> {
    let jobs = sqlx::query_as::<_, StorageCleanupJob>("SELECT id,storage_profile_id,object_key,attempts FROM storage_cleanup_jobs WHERE next_attempt_at <= NOW() ORDER BY created_at LIMIT 20").fetch_all(&state.db).await?;
    for job in jobs {
        let profile =
            sqlx::query_as::<_, StorageProfile>("SELECT * FROM storage_profiles WHERE id=$1")
                .bind(job.storage_profile_id)
                .fetch_optional(&state.db)
                .await?;
        match profile {
            Some(profile) => match storage::delete_object(state, &profile, &job.object_key).await {
                Ok(()) => {
                    sqlx::query("DELETE FROM storage_cleanup_jobs WHERE id=$1")
                        .bind(job.id)
                        .execute(&state.db)
                        .await?;
                }
                Err(error) => {
                    let seconds = 30_i64.saturating_mul(2_i64.pow(job.attempts.clamp(0, 8) as u32));
                    tracing::warn!(error=%error, cleanup_job_id=%job.id, "artifact cleanup will be retried");
                    sqlx::query("UPDATE storage_cleanup_jobs SET attempts=attempts+1, next_attempt_at=NOW() + ($1 * INTERVAL '1 second'), last_error=$2 WHERE id=$3")
                        .bind(seconds).bind(error.to_string()).bind(job.id).execute(&state.db).await?;
                }
            },
            None => {
                tracing::error!(cleanup_job_id=%job.id, "removing cleanup job whose storage profile no longer exists");
                sqlx::query("DELETE FROM storage_cleanup_jobs WHERE id=$1")
                    .bind(job.id)
                    .execute(&state.db)
                    .await?;
            }
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct UpdateQuery {
    current_version: String,
    channel: String,
    platform: String,
}
#[derive(Serialize)]
struct UpdateResponse {
    app_id: Uuid,
    release_id: Uuid,
    version: String,
    channel: String,
    release_notes: String,
    published_at: chrono::DateTime<Utc>,
    artifact: PublicArtifact,
}
#[derive(Serialize)]
struct PublicArtifact {
    id: Uuid,
    platform: String,
    file_name: String,
    size_bytes: i64,
    sha256: String,
    download_url: String,
}
async fn check_update(
    State(state): State<AppState>,
    Path(app_id): Path<Uuid>,
    Query(query): Query<UpdateQuery>,
) -> Result<Response, ApiError> {
    let current = Version::parse(&query.current_version)
        .map_err(|_| ApiError::validation("current_version must be valid SemVer"))?;
    if !matches!(query.channel.as_str(), "stable" | "beta") {
        return Err(ApiError::validation("channel must be stable or beta"));
    }
    validate_platform(&query.platform)?;
    let candidates = sqlx::query_as::<_, PublicReleaseArtifact>("SELECT r.app_id, r.id AS release_id, r.version, r.channel, r.release_notes, r.published_at, a.id AS artifact_id, a.original_file_name, a.size_bytes, a.sha256 FROM releases r JOIN artifacts a ON a.release_id=r.id JOIN apps app ON app.id=r.app_id WHERE r.app_id=$1 AND r.channel=$2 AND r.status='published' AND app.status='active' AND a.platform=$3")
        .bind(app_id).bind(&query.channel).bind(&query.platform).fetch_all(&state.db).await?;
    let latest = candidates
        .into_iter()
        .filter_map(|item| {
            Version::parse(&item.version)
                .ok()
                .filter(|v| v > &current)
                .map(|v| (v, item))
        })
        .max_by(|a, b| a.0.cmp(&b.0));
    let Some((_, item)) = latest else {
        return Ok(StatusCode::NO_CONTENT.into_response());
    };
    let body = UpdateResponse {
        app_id: item.app_id,
        release_id: item.release_id,
        version: item.version,
        channel: item.channel,
        release_notes: item.release_notes,
        published_at: item.published_at.unwrap_or_else(Utc::now),
        artifact: PublicArtifact {
            id: item.artifact_id,
            platform: query.platform,
            file_name: item.original_file_name,
            size_bytes: item.size_bytes,
            sha256: item.sha256,
            download_url: format!(
                "{}/api/public/artifacts/{}/download",
                state.config.base_url, item.artifact_id
            ),
        },
    };
    Ok(Json(body).into_response())
}

async fn download_artifact(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let artifact = sqlx::query_as::<_, Artifact>("SELECT a.* FROM artifacts a JOIN releases r ON r.id=a.release_id JOIN apps app ON app.id=r.app_id WHERE a.id=$1 AND r.status='published' AND app.status='active'")
        .bind(id).fetch_optional(&state.db).await?.ok_or_else(|| ApiError::not_found("Artifact not found"))?;
    let profile = sqlx::query_as::<_, StorageProfile>("SELECT * FROM storage_profiles WHERE id=$1")
        .bind(artifact.storage_profile_id)
        .fetch_one(&state.db)
        .await?;
    match storage::download_source(&state, &profile, &artifact.object_key).await? {
        DownloadSource::Redirect(url) => Ok(Redirect::temporary(&url).into_response()),
        DownloadSource::Local(path) => local_download(path, &artifact, headers).await,
    }
}

async fn local_download(
    path: PathBuf,
    artifact: &Artifact,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(ApiError::internal)?;
    let total = metadata.len();
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_range(v, total));
    let (start, end, status) = match range {
        Some((start, end)) => (start, end, StatusCode::PARTIAL_CONTENT),
        None => (0, total.saturating_sub(1), StatusCode::OK),
    };
    let length = if total == 0 { 0 } else { end - start + 1 };
    let mut file = File::open(path).await.map_err(ApiError::internal)?;
    file.seek(SeekFrom::Start(start))
        .await
        .map_err(ApiError::internal)?;
    let stream = ReaderStream::new(file.take(length));
    let safe_name = artifact.original_file_name.replace(['\r', '\n', '"'], "_");
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, &artifact.content_type)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, length)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{safe_name}\""),
        );
    if status == StatusCode::PARTIAL_CONTENT {
        response = response.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{total}"),
        );
    }
    response
        .body(Body::from_stream(stream))
        .map_err(ApiError::internal)
}

fn parse_range(value: &str, total: u64) -> Option<(u64, u64)> {
    let value = value.strip_prefix("bytes=")?;
    let part = value.split(',').next()?;
    let (left, right) = part.split_once('-')?;
    if total == 0 {
        return None;
    }
    match (left.parse::<u64>().ok(), right.parse::<u64>().ok()) {
        (Some(start), Some(end)) if start <= end && end < total => Some((start, end)),
        (Some(start), None) if start < total => Some((start, total - 1)),
        (None, Some(last)) if last > 0 => Some((total.saturating_sub(last), total - 1)),
        _ => None,
    }
}

async fn save_multipart_to_temp(
    state: &AppState,
    mut multipart: Multipart,
) -> Result<(String, String, String, PathBuf, u64, String), ApiError> {
    tokio::fs::create_dir_all(&state.config.temp_dir)
        .await
        .map_err(ApiError::internal)?;
    let mut platform = None;
    let mut file_meta = None;
    let path = state
        .config
        .temp_dir
        .join(format!("{}.upload", Uuid::new_v4()));
    let mut out = None;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    while let Some(mut field) = multipart.next_field().await.map_err(ApiError::internal)? {
        match field.name().unwrap_or_default() {
            "platform" => platform = Some(field.text().await.map_err(ApiError::internal)?),
            "file" => {
                if file_meta.is_some() {
                    return Err(ApiError::validation("Only one file field is allowed"));
                }
                let filename = field.file_name().unwrap_or("artifact.bin").to_owned();
                let content_type = field
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_owned();
                let file = File::create(&path).await.map_err(ApiError::internal)?;
                out = Some(file);
                file_meta = Some((filename, content_type));
                while let Some(chunk) = field.chunk().await.map_err(ApiError::internal)? {
                    size += chunk.len() as u64;
                    if size as usize > state.config.upload_max_bytes {
                        let _ = tokio::fs::remove_file(&path).await;
                        return Err(ApiError::validation(
                            "Upload exceeds configured maximum size",
                        ));
                    }
                    hasher.update(&chunk);
                    out.as_mut()
                        .unwrap()
                        .write_all(&chunk)
                        .await
                        .map_err(ApiError::internal)?;
                }
            }
            _ => {}
        }
    }
    if let Some(mut file) = out {
        file.flush().await.map_err(ApiError::internal)?;
    } else {
        return Err(ApiError::validation("file is required"));
    }
    let (filename, content_type) = file_meta.unwrap();
    let platform = match platform {
        Some(value) => value,
        None => {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(ApiError::validation("platform is required"));
        }
    };
    if let Err(error) = validate_platform(&platform) {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(error);
    }
    Ok((
        platform,
        filename,
        content_type,
        path,
        size,
        hex::encode(hasher.finalize()),
    ))
}

async fn authenticated(
    state: &AppState,
    jar: &CookieJar,
    headers: &HeaderMap,
) -> Result<(User, String), ApiError> {
    let (user, csrf) = auth::current_user(state, jar).await?;
    auth::require_csrf(headers, &csrf)?;
    Ok((user, csrf))
}
async fn app_for_actor(state: &AppState, id: Uuid, user: &User) -> Result<App, ApiError> {
    sqlx::query_as::<_, App>("SELECT * FROM apps WHERE id=$1 AND ($2='admin' OR owner_id=$3)")
        .bind(id)
        .bind(&user.role)
        .bind(user.id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::not_found("Application not found"))
}
async fn release_for_actor(state: &AppState, id: Uuid, user: &User) -> Result<Release, ApiError> {
    sqlx::query_as::<_, Release>("SELECT r.* FROM releases r JOIN apps app ON app.id=r.app_id WHERE r.id=$1 AND ($2='admin' OR app.owner_id=$3)").bind(id).bind(&user.role).bind(user.id).fetch_optional(&state.db).await?.ok_or_else(|| ApiError::not_found("Release not found"))
}
fn require_admin(user: &User) -> Result<(), ApiError> {
    if user.role == "admin" {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "Administrator permission required".into(),
        ))
    }
}
fn validate_username(value: &str) -> Result<(), ApiError> {
    let valid = value.len() >= 3
        && value.len() <= 64
        && value
            .bytes()
            .all(|v| v.is_ascii_alphanumeric() || v == b'_' || v == b'-');
    if valid {
        Ok(())
    } else {
        Err(ApiError::validation(
            "Username must be 3-64 characters of letters, numbers, _ or -",
        ))
    }
}
fn validate_password(value: &str) -> Result<(), ApiError> {
    if !value.is_empty() {
        Ok(())
    } else {
        Err(ApiError::validation("Password must not be empty"))
    }
}
fn validate_role(value: &str) -> Result<(), ApiError> {
    if matches!(value, "admin" | "user") {
        Ok(())
    } else {
        Err(ApiError::validation("role must be admin or user"))
    }
}
fn validate_app_input(value: &AppInput) -> Result<(), ApiError> {
    if value.name.trim().is_empty() || value.name.len() > 160 {
        return Err(ApiError::validation(
            "Application name must be 1-160 characters",
        ));
    }
    if value.description.len() > 10_000 {
        return Err(ApiError::validation(
            "Description must be at most 10000 characters",
        ));
    }
    Ok(())
}
fn validate_release_input(value: &ReleaseInput) -> Result<(), ApiError> {
    Version::parse(value.version.trim())
        .map_err(|_| ApiError::validation("version must be valid SemVer"))?;
    if !matches!(value.channel.as_str(), "stable" | "beta") {
        return Err(ApiError::validation("channel must be stable or beta"));
    }
    if value.release_notes.len() > 50_000 {
        return Err(ApiError::validation("Release notes are too long"));
    }
    Ok(())
}
fn validate_platform(value: &str) -> Result<(), ApiError> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|v| {
            v.is_ascii_lowercase() || v.is_ascii_digit() || matches!(v, b'.' | b'_' | b'-')
        });
    if valid {
        Ok(())
    } else {
        Err(ApiError::validation(
            "platform must be a 1-64 character lowercase slug",
        ))
    }
}
fn sha256_hex(value: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_platform_slugs() {
        assert!(validate_platform("windows-x64").is_ok());
        assert!(validate_platform("Android").is_err());
    }
    #[test]
    fn understands_ranges() {
        assert_eq!(parse_range("bytes=10-19", 100), Some((10, 19)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=100-", 100), None);
    }
    #[test]
    fn semver_is_required() {
        assert!(Version::parse("1.2.3").is_ok());
        assert!(Version::parse("2024-1").is_err());
    }
    #[test]
    fn validates_storage_migration_directions() {
        assert!(validate_migration_direction("s3", "s3").is_ok());
        assert!(validate_migration_direction("s3", "local").is_ok());
        assert!(validate_migration_direction("local", "s3").is_ok());
        assert!(validate_migration_direction("local", "local").is_err());
        assert!(validate_migration_direction("unknown", "s3").is_err());
    }
    #[test]
    fn password_length_is_unrestricted_but_empty_passwords_are_rejected() {
        assert!(validate_password("a").is_ok());
        assert!(validate_password(&"a".repeat(257)).is_ok());
        assert!(validate_password("").is_err());
    }
    #[test]
    fn migration_retries_use_exponential_backoff() {
        assert_eq!(migration_retry_delay_seconds(1), 30);
        assert_eq!(migration_retry_delay_seconds(2), 60);
        assert_eq!(migration_retry_delay_seconds(4), 240);
    }
}
