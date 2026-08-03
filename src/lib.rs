pub mod bot;
pub mod calibre;
pub mod client;
pub mod config;
pub mod dashboard;
pub mod db;
pub mod email;
pub mod models;
pub mod tor;

pub use bot::{start_bot, AppState};
pub use calibre::start_calibre_server;
pub use client::ZLibraryClient;
pub use config::{
    AuthConfig, CalibreSettings, Config, DashboardSettings, SmtpSettings, StorageSettings,
    TelegramSettings, TorSettings,
};
pub use dashboard::start_dashboard_server;
pub use db::Database;
pub use email::{extract_epub_cover, generate_kindle_test_epub, EmailSender};
pub use models::{Book, Credentials, SearchQuery, SessionTokens, UserProfile};
pub use tor::{TorConfig, TorMode, DEFAULT_SOCKS5_PROXY, DEFAULT_ZLIB_ONION, FALLBACK_ZLIB_ONION};


