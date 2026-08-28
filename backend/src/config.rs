use std::{env, net::SocketAddr, path::PathBuf};

use base64::{Engine, engine::general_purpose::STANDARD};

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: SocketAddr,
    pub base_url: String,
    pub initial_admin_username: Option<String>,
    pub initial_admin_password: Option<String>,
    pub settings_master_key: [u8; 32],
    pub upload_max_bytes: usize,
    pub cookie_secure: bool,
    pub temp_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        load_dotenv()?;
        let database_url = required("DATABASE_URL")?;
        let bind_addr = env_or("APP_BIND", "0.0.0.0:8080")
            .parse()
            .map_err(|_| "APP_BIND must be a socket address")?;
        let base_url = env_or("APP_BASE_URL", "http://localhost:8080")
            .trim_end_matches('/')
            .to_owned();
        let master_key_text = required("SETTINGS_MASTER_KEY")?;
        let bytes = STANDARD
            .decode(master_key_text)
            .map_err(|_| "SETTINGS_MASTER_KEY must be base64")?;
        let settings_master_key: [u8; 32] = bytes
            .try_into()
            .map_err(|_| "SETTINGS_MASTER_KEY must decode to exactly 32 bytes")?;
        let upload_max_bytes = env_or("UPLOAD_MAX_BYTES", "2147483648")
            .parse()
            .map_err(|_| "UPLOAD_MAX_BYTES must be an integer")?;
        let cookie_secure = env_or("COOKIE_SECURE", "false")
            .parse()
            .map_err(|_| "COOKIE_SECURE must be true or false")?;
        let temp_dir = env::var("UPLOAD_TEMP_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| env::temp_dir().join("update-service"));

        Ok(Self {
            database_url,
            bind_addr,
            base_url,
            initial_admin_username: env::var("INITIAL_ADMIN_USERNAME")
                .ok()
                .filter(|v| !v.is_empty()),
            initial_admin_password: env::var("INITIAL_ADMIN_PASSWORD")
                .ok()
                .filter(|v| !v.is_empty()),
            settings_master_key,
            upload_max_bytes,
            cookie_secure,
            temp_dir,
        })
    }
}

fn load_dotenv() -> Result<(), String> {
    let current_dir = env::current_dir().map_err(|error| error.to_string())?;
    let paths = [current_dir.join(".env"), current_dir.join("../.env")];

    for path in paths {
        if path.is_file() {
            dotenvy::from_path(&path)
                .map_err(|error| format!("failed to load {}: {error}", path.display()))?;
            return Ok(());
        }
    }

    Ok(())
}

fn required(name: &'static str) -> Result<String, String> {
    env::var(name).map_err(|_| format!("{name} is required"))
}

fn env_or(name: &'static str, fallback: &'static str) -> String {
    env::var(name).unwrap_or_else(|_| fallback.to_owned())
}
