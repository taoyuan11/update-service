use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub role: String,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct App {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub owner_id: Uuid,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Release {
    pub id: Uuid,
    pub app_id: Uuid,
    pub version: String,
    pub channel: String,
    pub release_notes: String,
    pub status: String,
    pub published_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Artifact {
    pub id: Uuid,
    pub release_id: Uuid,
    pub platform: String,
    pub original_file_name: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub storage_profile_id: Uuid,
    pub object_key: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct StorageProfile {
    pub id: Uuid,
    pub name: String,
    pub backend: String,
    pub config: serde_json::Value,
    pub secret_encrypted: Option<String>,
    pub is_active: bool,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct StorageProfileResponse {
    pub id: Uuid,
    pub name: String,
    pub backend: String,
    pub config: serde_json::Value,
    pub has_secret: bool,
    pub is_active: bool,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub artifact_count: i64,
    pub artifact_bytes: i64,
}

impl From<StorageProfile> for StorageProfileResponse {
    fn from(value: StorageProfile) -> Self {
        Self {
            id: value.id,
            name: value.name,
            backend: value.backend,
            config: value.config,
            has_secret: value.secret_encrypted.is_some(),
            is_active: value.is_active,
            created_by: value.created_by,
            created_at: value.created_at,
            artifact_count: 0,
            artifact_bytes: 0,
        }
    }
}

#[derive(Debug, FromRow)]
pub struct PublicReleaseArtifact {
    pub app_id: Uuid,
    pub release_id: Uuid,
    pub version: String,
    pub channel: String,
    pub release_notes: String,
    pub published_at: Option<DateTime<Utc>>,
    pub artifact_id: Uuid,
    pub original_file_name: String,
    pub size_bytes: i64,
    pub sha256: String,
}

#[derive(Debug, FromRow)]
pub struct StorageCleanupJob {
    pub id: Uuid,
    pub storage_profile_id: Uuid,
    pub object_key: String,
    pub attempts: i32,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct StorageMigration {
    pub id: Uuid,
    pub source_profile_id: Option<Uuid>,
    pub destination_profile_id: Option<Uuid>,
    pub source_profile_name: String,
    pub destination_profile_name: String,
    pub source_backend: String,
    pub destination_backend: String,
    pub status: String,
    pub total_objects: i64,
    pub completed_objects: i64,
    pub failed_objects: i64,
    pub skipped_objects: i64,
    pub total_bytes: i64,
    pub completed_bytes: i64,
    pub cancel_requested: bool,
    pub last_error: Option<String>,
    pub requested_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct StorageMigrationItem {
    pub id: Uuid,
    pub migration_id: Uuid,
    pub artifact_id: Option<Uuid>,
    pub artifact_id_snapshot: Uuid,
    pub object_key: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub content_type: String,
    pub status: String,
    pub attempts: i32,
    pub bytes_copied: i64,
    pub next_attempt_at: DateTime<Utc>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}
