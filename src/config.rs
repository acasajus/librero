use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;

/// Allowed Telegram User mapping configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramUserConfig {
    pub user_id: i64,
    pub username: Option<String>,
    pub email: String,
}

/// Telegram Bot settings in config.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramSettings {
    pub bot_token: String,
    pub allowed_users: Vec<TelegramUserConfig>,
}

impl Default for TelegramSettings {
    fn default() -> Self {
        Self {
            bot_token: "YOUR_TELEGRAM_BOT_TOKEN".to_string(),
            allowed_users: vec![TelegramUserConfig {
                user_id: 123456789,
                username: Some("myusername".to_string()),
                email: "myemail@kindle.com".to_string(),
            }],
        }
    }
}

/// Gmail / SMTP Server credentials in config.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmtpSettings {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub from_email: String,
}

impl Default for SmtpSettings {
    fn default() -> Self {
        Self {
            host: "smtp.gmail.com".to_string(),
            port: 587,
            username: "your_gmail@gmail.com".to_string(),
            password: "your_app_password".to_string(),
            from_email: "your_gmail@gmail.com".to_string(),
        }
    }
}

/// Local & Turso storage settings in config.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageSettings {
    pub download_dir: String,
    pub turso_db_path: String,
}

impl Default for StorageSettings {
    fn default() -> Self {
        Self {
            download_dir: "./downloads".to_string(),
            turso_db_path: "librero.db".to_string(),
        }
    }
}

/// Z-Library Authentication settings in config.toml
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthConfig {
    pub email: Option<String>,
    pub password: Option<String>,
    pub remix_userid: Option<String>,
    pub remix_userkey: Option<String>,
}

/// Tor network connection settings in config.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorSettings {
    pub mode: String,
    pub proxy_url: String,
    pub onion_address: String,
    pub fallback_onion_addresses: Vec<String>,
    pub connect_timeout_seconds: u64,
}

impl Default for TorSettings {
    fn default() -> Self {
        Self {
            mode: "auto".to_string(),
            proxy_url: "socks5h://127.0.0.1:9050".to_string(),
            onion_address: "http://loginzlib2vrak5zzpcocc3ouizykn6k5qecgj2tzlnab5wcbqhembyd.onion".to_string(),
            fallback_onion_addresses: vec![
                "http://bookszlibb74ugqojhzhg2a63w5i2atv5bqarulgczawnbmsb6s6qead.onion".to_string(),
            ],
            connect_timeout_seconds: 45,
        }
    }
}

/// Top-level configuration object stored in config.toml
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub auth: AuthConfig,
    pub tor: TorSettings,
    pub telegram: TelegramSettings,
    pub smtp: SmtpSettings,
    pub storage: StorageSettings,
}

impl Config {
    /// Attempt to load configuration from given path, current directory `config.toml`,
    /// or `~/.config/librero/config.toml`. Returns default configuration if none is found.
    pub fn load(explicit_path: Option<&PathBuf>) -> Result<Self> {
        let candidates = match explicit_path {
            Some(p) => vec![p.clone()],
            None => {
                let mut paths = vec![PathBuf::from("config.toml")];
                if let Some(user_config_dir) = dirs::config_dir() {
                    paths.push(user_config_dir.join("librero").join("config.toml"));
                }
                paths
            }
        };

        for path in candidates {
            if path.exists() {
                info!("Loading configuration from {:?}", path);
                let content = fs::read_to_string(&path)
                    .with_context(|| format!("Failed to read config file at {:?}", path))?;
                let config: Config = toml::from_str(&content)
                    .with_context(|| format!("Failed to parse TOML config at {:?}", path))?;
                return Ok(config);
            }
        }

        info!("No configuration file found. Using default settings.");
        Ok(Config::default())
    }

    /// Find configured recipient email for a specific Telegram user ID
    pub fn find_user_email(&self, telegram_user_id: i64) -> Option<String> {
        self.telegram
            .allowed_users
            .iter()
            .find(|u| u.user_id == telegram_user_id)
            .map(|u| u.email.clone())
    }

    /// Check if a Telegram user ID is authorized
    pub fn is_user_allowed(&self, telegram_user_id: i64) -> bool {
        self.telegram
            .allowed_users
            .iter()
            .any(|u| u.user_id == telegram_user_id)
    }

    /// Generate an example config.toml string
    pub fn default_toml_template() -> String {
        r#"# Librero Daemon Configuration File

[auth]
# Z-Library Account Credentials
email = "your_zlibrary_email@example.com"
password = "your_zlibrary_password"

# Session Tokens (optional, auto-generated on login)
# remix_userid = ""
# remix_userkey = ""

[telegram]
# Telegram Bot API Token from @BotFather
bot_token = "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ"

# List of authorized Telegram users and their target SMTP recipient email addresses
[[telegram.allowed_users]]
user_id = 123456789             # Telegram numerical User ID
username = "myuser"             # Optional reference username
email = "mykindle@kindle.com"   # Email address where books will be sent

[smtp]
# Gmail SMTP Configuration
host = "smtp.gmail.com"
port = 587
username = "your_gmail@gmail.com"
password = "your_gmail_app_password"  # Gmail App Password (not your main password)
from_email = "your_gmail@gmail.com"

[storage]
# Local directory to store downloaded books (subdirectories created per user)
download_dir = "./downloads"

# Local SQLite / Turso Database Path
turso_db_path = "librero.db"

[tor]
# Connection Mode: "auto" (local SOCKS5 proxy -> fallback to embedded Arti client), "socks5", or "embedded"
mode = "auto"
proxy_url = "socks5h://127.0.0.1:9050"
onion_address = "http://loginzlib2vrak5zzpcocc3ouizykn6k5qecgj2tzlnab5wcbqhembyd.onion"

fallback_onion_addresses = [
    "http://bookszlibb74ugqojhzhg2a63w5i2atv5bqarulgczawnbmsb6s6qead.onion"
]

connect_timeout_seconds = 45
"#
        .to_string()
    }

    /// Save current configuration to a TOML file
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        let content = toml::to_string_pretty(self)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)?;
        Ok(())
    }
}
