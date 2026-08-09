use std::io::Write;
use std::net::SocketAddr;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Runtime configuration, sourced from environment variables with sensible
/// defaults for local development.
#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub database_url: String,
    /// Base URL used when building shareable page links, e.g. `https://books.example.com`.
    pub public_base_url: String,
    /// Set the `Secure` attribute on cookies (enable behind HTTPS).
    pub secure_cookies: bool,
    /// Server-side secret used for ISBN blind indexes. Never stored in the DB.
    pub pepper: [u8; 32],
    /// Disabled in tests.
    pub rate_limiting: bool,
    pub static_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Config> {
        let bind: SocketAddr = env_or("BIND", "127.0.0.1:8080").parse()?;
        let database_url = env_or("DATABASE_URL", "sqlite://anonbookrec.db");
        let public_base_url = std::env::var("PUBLIC_BASE_URL")
            .unwrap_or_else(|_| format!("http://{bind}"))
            .trim_end_matches('/')
            .to_string();
        let secure_cookies = env_or("SECURE_COOKIES", "0") == "1";
        let pepper = load_pepper()?;
        let static_dir = PathBuf::from(env_or("STATIC_DIR", "static"));
        Ok(Config {
            bind,
            database_url,
            public_base_url,
            secure_cookies,
            pepper,
            rate_limiting: true,
            static_dir,
        })
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Pepper comes from `PEPPER` (64 hex chars) or a `.pepper` file (created on
/// first run with mode 0600) so blind indexes survive restarts.
fn load_pepper() -> anyhow::Result<[u8; 32]> {
    if let Ok(hex_str) = std::env::var("PEPPER") {
        let bytes = hex::decode(hex_str.trim())?;
        return bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("PEPPER must be 64 hex characters (32 bytes)"));
    }

    let path = std::env::var("PEPPER_FILE").unwrap_or_else(|_| ".pepper".to_string());
    let path = Path::new(&path);
    if path.exists() {
        let content = std::fs::read_to_string(path)?;
        let bytes = hex::decode(content.trim())?;
        return bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("pepper file must contain 64 hex characters"));
    }

    let mut pepper = [0u8; 32];
    use rand::RngCore;
    rand::rngs::OsRng.fill_bytes(&mut pepper);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(hex::encode(pepper).as_bytes())?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    tracing::info!("generated new pepper at {}", path.display());
    Ok(pepper)
}
