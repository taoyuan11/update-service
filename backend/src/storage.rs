use std::{
    io::ErrorKind,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use aws_credential_types::Credentials;
use aws_sdk_s3::{
    Client as S3Client,
    config::{BehaviorVersion, Region},
    presigning::PresigningConfig,
    primitives::ByteStream,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use crate::{AppState, error::ApiError, models::StorageProfile};

#[derive(Debug, Deserialize, Serialize)]
pub struct LocalConfig {
    pub root: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct S3Config {
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub path_style: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct S3Secret {
    pub access_key: String,
    pub secret_key: String,
}

pub enum DownloadSource {
    Local(PathBuf),
    Redirect(String),
}

/// Shared advisory lock used for storage profile cutovers and artifact
/// reference finalization. Keeping this in the storage module prevents the
/// upload and migration paths from accidentally using different locks.
pub const STORAGE_MUTATION_LOCK_KEY: i64 = 4_827_193_604;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectStats {
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferOutcome {
    Copied,
    AlreadyPresent,
}

pub fn encrypt_secret(key: &[u8; 32], value: &str) -> Result<String, ApiError> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(ApiError::internal)?;
    let mut nonce_bytes = [0u8; 12];
    rand::rng().fill(&mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), value.as_bytes())
        .map_err(ApiError::internal)?;
    let mut packed = nonce_bytes.to_vec();
    packed.extend(ciphertext);
    Ok(STANDARD.encode(packed))
}

pub fn decrypt_secret(key: &[u8; 32], value: &str) -> Result<String, ApiError> {
    let packed = STANDARD
        .decode(value)
        .map_err(|_| ApiError::Internal("Stored credential cannot be decoded".into()))?;
    if packed.len() < 13 {
        return Err(ApiError::Internal("Stored credential is malformed".into()));
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(ApiError::internal)?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&packed[..12]), &packed[12..])
        .map_err(|_| ApiError::Internal("Stored credential cannot be decrypted".into()))?;
    String::from_utf8(plaintext).map_err(ApiError::internal)
}

pub async fn upload_temp_file_with_metadata(
    state: &AppState,
    profile: &StorageProfile,
    object_key: &str,
    source: &Path,
    content_type: Option<&str>,
    sha256: Option<&str>,
) -> Result<(), ApiError> {
    match profile.backend.as_str() {
        "local" => {
            let config: LocalConfig = serde_json::from_value(profile.config.clone())
                .map_err(|_| ApiError::validation("Invalid local storage configuration"))?;
            let destination = checked_local_path(&config.root, object_key)?;
            if let Some(parent) = destination.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(ApiError::internal)?;
            }
            tokio::fs::copy(source, &destination)
                .await
                .map_err(ApiError::internal)?;
            Ok(())
        }
        "s3" => {
            let (client, config) = s3_client(state, profile).await?;
            let key = s3_key(&config.prefix, object_key);
            let body = ByteStream::from_path(source)
                .await
                .map_err(ApiError::internal)?;
            let mut request = client
                .put_object()
                .bucket(config.bucket)
                .key(key)
                .body(body);
            if let Some(content_type) = content_type.filter(|v| !v.trim().is_empty()) {
                request = request.content_type(content_type);
            }
            if let Some(sha256) = sha256.filter(|v| !v.trim().is_empty()) {
                request = request.metadata("sha256", sha256);
            }
            request.send().await.map_err(ApiError::internal)?;
            Ok(())
        }
        _ => Err(ApiError::internal("Unknown storage backend")),
    }
}

/// Copy one logical object between any two supported storage profiles.
///
/// The service deliberately relays S3-to-S3 copies through a temporary file.
/// This keeps credentials isolated to their respective clients and works for
/// S3-compatible providers that do not implement cross-bucket CopyObject.
pub async fn migrate_object(
    state: &AppState,
    source: &StorageProfile,
    destination: &StorageProfile,
    object_key: &str,
    expected_size: i64,
    expected_sha256: &str,
    content_type: &str,
) -> Result<TransferOutcome, ApiError> {
    let expected_size = u64::try_from(expected_size)
        .map_err(|_| ApiError::validation("Migration object size cannot be negative"))?;
    tokio::fs::create_dir_all(&state.config.temp_dir)
        .await
        .map_err(ApiError::internal)?;
    let temp_path = state
        .config
        .temp_dir
        .join(format!("{}.migration", Uuid::new_v4()));

    let result = async {
        let source_stats = materialize_source(state, source, object_key, &temp_path).await?;
        if source_stats.size_bytes != expected_size || source_stats.sha256 != expected_sha256 {
            return Err(ApiError::Conflict(format!(
                "Source object does not match artifact metadata: {object_key}"
            )));
        }

        let already_present = destination_matches(
            state,
            destination,
            object_key,
            expected_size,
            expected_sha256,
        )
        .await?;
        if !already_present {
            upload_temp_file_with_metadata(
                state,
                destination,
                object_key,
                &temp_path,
                Some(content_type),
                Some(expected_sha256),
            )
            .await?;
        }

        let destination_stats = object_stats(state, destination, object_key).await?;
        if destination_stats.size_bytes != expected_size
            || destination_stats.sha256 != expected_sha256
        {
            return Err(ApiError::Conflict(format!(
                "Destination object failed integrity verification: {object_key}"
            )));
        }
        Ok(if already_present {
            TransferOutcome::AlreadyPresent
        } else {
            TransferOutcome::Copied
        })
    }
    .await;

    // A migration temp file is never part of the persistent storage layout.
    if let Err(error) = tokio::fs::remove_file(&temp_path).await {
        if error.kind() != ErrorKind::NotFound {
            tracing::warn!(error = %error, path = %temp_path.display(), "failed to remove migration temp file");
        }
    }
    result
}

async fn materialize_source(
    state: &AppState,
    profile: &StorageProfile,
    object_key: &str,
    destination: &Path,
) -> Result<ObjectStats, ApiError> {
    match profile.backend.as_str() {
        "local" => {
            let config: LocalConfig = serde_json::from_value(profile.config.clone())
                .map_err(|_| ApiError::validation("Invalid local storage configuration"))?;
            let source = checked_local_path(&config.root, object_key)?;
            let mut input = tokio::fs::File::open(&source).await.map_err(|error| {
                if error.kind() == ErrorKind::NotFound {
                    ApiError::not_found("Migration source object not found")
                } else {
                    ApiError::internal(error)
                }
            })?;
            let mut output = tokio::fs::File::create(destination)
                .await
                .map_err(ApiError::internal)?;
            let stats = copy_and_hash(&mut input, &mut output).await?;
            output.flush().await.map_err(ApiError::internal)?;
            Ok(stats)
        }
        "s3" => {
            let (client, config) = s3_client(state, profile).await?;
            let response = client
                .get_object()
                .bucket(&config.bucket)
                .key(s3_key(&config.prefix, object_key))
                .send()
                .await
                .map_err(|error| {
                    if error
                        .as_service_error()
                        .is_some_and(|service| service.is_no_such_key())
                    {
                        ApiError::not_found("Migration source object not found")
                    } else {
                        ApiError::internal(error)
                    }
                })?;
            let mut body = response.body;
            let mut output = tokio::fs::File::create(destination)
                .await
                .map_err(ApiError::internal)?;
            let mut hasher = Sha256::new();
            let mut size = 0u64;
            while let Some(chunk) = body.next().await {
                let chunk = chunk.map_err(ApiError::internal)?;
                size = size.saturating_add(chunk.len() as u64);
                hasher.update(&chunk);
                output.write_all(&chunk).await.map_err(ApiError::internal)?;
            }
            output.flush().await.map_err(ApiError::internal)?;
            Ok(ObjectStats {
                size_bytes: size,
                sha256: hex::encode(hasher.finalize()),
            })
        }
        _ => Err(ApiError::validation("Unsupported migration source backend")),
    }
}

async fn destination_matches(
    state: &AppState,
    profile: &StorageProfile,
    object_key: &str,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<bool, ApiError> {
    match object_stats_optional(state, profile, object_key).await? {
        Some(stats) => Ok(stats.size_bytes == expected_size && stats.sha256 == expected_sha256),
        None => Ok(false),
    }
}

async fn object_stats(
    state: &AppState,
    profile: &StorageProfile,
    object_key: &str,
) -> Result<ObjectStats, ApiError> {
    object_stats_optional(state, profile, object_key)
        .await?
        .ok_or_else(|| ApiError::not_found("Migration destination object not found"))
}

async fn object_stats_optional(
    state: &AppState,
    profile: &StorageProfile,
    object_key: &str,
) -> Result<Option<ObjectStats>, ApiError> {
    match profile.backend.as_str() {
        "local" => {
            let config: LocalConfig = serde_json::from_value(profile.config.clone())
                .map_err(|_| ApiError::validation("Invalid local storage configuration"))?;
            let path = checked_local_path(&config.root, object_key)?;
            match tokio::fs::metadata(&path).await {
                Ok(metadata) if metadata.is_file() => Ok(Some(hash_file(&path).await?)),
                Ok(_) => Ok(None),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
                Err(error) => Err(ApiError::internal(error)),
            }
        }
        "s3" => {
            let (client, config) = s3_client(state, profile).await?;
            let head = match client
                .head_object()
                .bucket(&config.bucket)
                .key(s3_key(&config.prefix, object_key))
                .send()
                .await
            {
                Ok(value) => value,
                Err(error)
                    if error
                        .as_service_error()
                        .is_some_and(|service| service.is_not_found()) =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(ApiError::internal(error)),
            };
            let size_hint = head.content_length.unwrap_or_default();
            let response = client
                .get_object()
                .bucket(&config.bucket)
                .key(s3_key(&config.prefix, object_key))
                .send()
                .await
                .map_err(ApiError::internal)?;
            let mut body = response.body;
            let mut hasher = Sha256::new();
            let mut size = 0u64;
            while let Some(chunk) = body.next().await {
                let chunk = chunk.map_err(ApiError::internal)?;
                size = size.saturating_add(chunk.len() as u64);
                hasher.update(&chunk);
            }
            // A provider may omit Content-Length; the streamed byte count is
            // authoritative in that case.
            if size_hint >= 0 && size_hint as u64 != size {
                tracing::warn!(
                    object_key,
                    size_hint,
                    size,
                    "S3 object length changed during migration verification"
                );
            }
            Ok(Some(ObjectStats {
                size_bytes: size,
                sha256: hex::encode(hasher.finalize()),
            }))
        }
        _ => Err(ApiError::validation(
            "Unsupported migration destination backend",
        )),
    }
}

async fn hash_file(path: &Path) -> Result<ObjectStats, ApiError> {
    let mut input = tokio::fs::File::open(path)
        .await
        .map_err(ApiError::internal)?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = input.read(&mut buffer).await.map_err(ApiError::internal)?;
        if read == 0 {
            break;
        }
        size = size.saturating_add(read as u64);
        hasher.update(&buffer[..read]);
    }
    Ok(ObjectStats {
        size_bytes: size,
        sha256: hex::encode(hasher.finalize()),
    })
}

async fn copy_and_hash(
    input: &mut tokio::fs::File,
    output: &mut tokio::fs::File,
) -> Result<ObjectStats, ApiError> {
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = input.read(&mut buffer).await.map_err(ApiError::internal)?;
        if read == 0 {
            break;
        }
        size = size.saturating_add(read as u64);
        hasher.update(&buffer[..read]);
        output
            .write_all(&buffer[..read])
            .await
            .map_err(ApiError::internal)?;
    }
    Ok(ObjectStats {
        size_bytes: size,
        sha256: hex::encode(hasher.finalize()),
    })
}

pub async fn delete_object(
    state: &AppState,
    profile: &StorageProfile,
    object_key: &str,
) -> Result<(), ApiError> {
    match profile.backend.as_str() {
        "local" => {
            let config: LocalConfig = serde_json::from_value(profile.config.clone())
                .map_err(|_| ApiError::validation("Invalid local storage configuration"))?;
            let destination = checked_local_path(&config.root, object_key)?;
            match tokio::fs::remove_file(destination).await {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(ApiError::internal(e)),
            }
        }
        "s3" => {
            let (client, config) = s3_client(state, profile).await?;
            client
                .delete_object()
                .bucket(config.bucket)
                .key(s3_key(&config.prefix, object_key))
                .send()
                .await
                .map_err(ApiError::internal)?;
            Ok(())
        }
        _ => Err(ApiError::internal("Unknown storage backend")),
    }
}

pub async fn download_source(
    state: &AppState,
    profile: &StorageProfile,
    object_key: &str,
) -> Result<DownloadSource, ApiError> {
    match profile.backend.as_str() {
        "local" => {
            let config: LocalConfig = serde_json::from_value(profile.config.clone())
                .map_err(|_| ApiError::validation("Invalid local storage configuration"))?;
            let path = checked_local_path(&config.root, object_key)?;
            if !path.is_file() {
                return Err(ApiError::not_found("Artifact file not found"));
            }
            Ok(DownloadSource::Local(path))
        }
        "s3" => {
            let (client, config) = s3_client(state, profile).await?;
            let signing = PresigningConfig::expires_in(Duration::from_secs(300))
                .map_err(ApiError::internal)?;
            let request = client
                .get_object()
                .bucket(config.bucket)
                .key(s3_key(&config.prefix, object_key))
                .presigned(signing)
                .await
                .map_err(ApiError::internal)?;
            Ok(DownloadSource::Redirect(request.uri().to_string()))
        }
        _ => Err(ApiError::internal("Unknown storage backend")),
    }
}

pub async fn test_profile(state: &AppState, profile: &StorageProfile) -> Result<(), ApiError> {
    match profile.backend.as_str() {
        "local" => {
            let config: LocalConfig = serde_json::from_value(profile.config.clone())
                .map_err(|_| ApiError::validation("Invalid local storage configuration"))?;
            tokio::fs::create_dir_all(config.root)
                .await
                .map_err(ApiError::internal)
        }
        "s3" => {
            let (client, config) = s3_client(state, profile).await?;
            client
                .head_bucket()
                .bucket(config.bucket)
                .send()
                .await
                .map(|_| ())
                .map_err(ApiError::internal)
        }
        _ => Err(ApiError::validation("Unsupported storage backend")),
    }
}

pub fn validate_profile(
    backend: &str,
    config: &serde_json::Value,
    secret: Option<&str>,
) -> Result<(), ApiError> {
    match backend {
        "local" => {
            let parsed: LocalConfig = serde_json::from_value(config.clone())
                .map_err(|_| ApiError::validation("Local storage needs a root path"))?;
            if parsed.root.trim().is_empty() {
                return Err(ApiError::validation("Local storage root cannot be empty"));
            }
        }
        "s3" => {
            let parsed: S3Config = serde_json::from_value(config.clone())
                .map_err(|_| ApiError::validation("Invalid S3 configuration"))?;
            if parsed.region.trim().is_empty() || parsed.bucket.trim().is_empty() {
                return Err(ApiError::validation("S3 requires region and bucket"));
            }
            let secret: S3Secret = serde_json::from_str(
                secret.ok_or_else(|| ApiError::validation("S3 requires access credentials"))?,
            )
            .map_err(|_| ApiError::validation("Invalid S3 credentials"))?;
            if secret.access_key.trim().is_empty() || secret.secret_key.trim().is_empty() {
                return Err(ApiError::validation("S3 credentials cannot be empty"));
            }
        }
        _ => return Err(ApiError::validation("backend must be local or s3")),
    }
    Ok(())
}

fn checked_local_path(root: &str, object_key: &str) -> Result<PathBuf, ApiError> {
    let relative = Path::new(object_key);
    if relative.is_absolute()
        || object_key.is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ApiError::validation("Invalid object key"));
    }
    Ok(PathBuf::from(root).join(relative))
}

fn s3_key(prefix: &str, object_key: &str) -> String {
    let prefix = prefix.trim_matches('/');
    if prefix.is_empty() {
        object_key.to_owned()
    } else {
        format!("{prefix}/{object_key}")
    }
}

async fn s3_client(
    state: &AppState,
    profile: &StorageProfile,
) -> Result<(S3Client, S3Config), ApiError> {
    let config: S3Config = serde_json::from_value(profile.config.clone())
        .map_err(|_| ApiError::validation("Invalid S3 configuration"))?;
    let encrypted = profile
        .secret_encrypted
        .as_deref()
        .ok_or_else(|| ApiError::Internal("S3 credentials are missing".into()))?;
    let secret: S3Secret = serde_json::from_str(&decrypt_secret(
        &state.config.settings_master_key,
        encrypted,
    )?)
    .map_err(ApiError::internal)?;
    let mut loader = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new(config.region.clone()))
        .credentials_provider(Credentials::new(
            secret.access_key,
            secret.secret_key,
            None,
            None,
            "update-service",
        ));
    if let Some(endpoint) = &config.endpoint {
        if !endpoint.trim().is_empty() {
            loader = loader.endpoint_url(endpoint);
        }
    }
    let shared = loader.load().await;
    let client_config = aws_sdk_s3::config::Builder::from(&shared)
        .force_path_style(config.path_style)
        .build();
    Ok((S3Client::from_conf(client_config), config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encryption_round_trip() {
        let key = [7; 32];
        let encrypted = encrypt_secret(&key, "top-secret").unwrap();
        assert_ne!(encrypted, "top-secret");
        assert_eq!(decrypt_secret(&key, &encrypted).unwrap(), "top-secret");
    }

    #[test]
    fn local_object_keys_cannot_escape_the_root() {
        assert_eq!(
            checked_local_path("/srv/artifacts", "artifacts/app/file").unwrap(),
            PathBuf::from("/srv/artifacts/artifacts/app/file")
        );
        assert!(checked_local_path("/srv/artifacts", "../secret").is_err());
        assert!(checked_local_path("/srv/artifacts", "artifacts/../secret").is_err());
        assert!(checked_local_path("/srv/artifacts", "/etc/passwd").is_err());
        assert!(checked_local_path("/srv/artifacts", "").is_err());
    }

    #[test]
    fn s3_prefix_is_normalized() {
        assert_eq!(
            s3_key("/updates/", "artifacts/one"),
            "updates/artifacts/one"
        );
        assert_eq!(s3_key("", "artifacts/one"), "artifacts/one");
    }

    #[tokio::test]
    async fn local_copy_computes_size_and_sha256() {
        let directory = tempfile::tempdir().unwrap();
        let input_path = directory.path().join("input");
        let output_path = directory.path().join("output");
        std::fs::write(&input_path, b"migration payload").unwrap();
        let mut input = tokio::fs::File::open(&input_path).await.unwrap();
        let mut output = tokio::fs::File::create(&output_path).await.unwrap();
        let stats = copy_and_hash(&mut input, &mut output).await.unwrap();
        output.flush().await.unwrap();

        assert_eq!(stats.size_bytes, 17);
        assert_eq!(
            stats.sha256,
            hex::encode(Sha256::digest(b"migration payload"))
        );
        assert_eq!(std::fs::read(output_path).unwrap(), b"migration payload");
    }
}
