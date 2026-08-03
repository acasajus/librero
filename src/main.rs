use anyhow::Result;
use clap::Parser;
use librero::{
    start_bot, start_calibre_server, start_dashboard_server, AppState, Config, Database,
    EmailSender, TorConfig, TorMode, ZLibraryClient,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::sync::Mutex;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "librero")]
#[command(about = "Z-Library Tor Telegram Bot & Automated Book Delivery Daemon", long_about = None)]
struct Cli {
    /// Path to TOML configuration file (defaults to ./config.toml or ~/.config/librero/config.toml)
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Override Tor connection mode ("auto", "socks5", "embedded")
    #[arg(short, long)]
    mode: Option<String>,

    /// Override Tor SOCKS5 proxy URL
    #[arg(short, long)]
    proxy: Option<String>,

    /// Override Calibre Content Server port (defaults to 8080 or config.toml [calibre] port)
    #[arg(short = 'P', long = "calibre-port")]
    calibre_port: Option<u16>,

    /// Override Admin Web Dashboard port (defaults to 8081 or config.toml [dashboard] port)
    #[arg(short = 'D', long = "dashboard-port")]
    dashboard_port: Option<u16>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let cli = Cli::parse();
    info!("Starting Librero Telegram Service Daemon...");

    // 1. Load Configuration File
    let cfg = Config::load(cli.config.as_ref())?;

    // 2. Initialize Storage Directories & Turso Database
    if let Err(e) = fs::create_dir_all(&cfg.storage.download_dir).await {
        warn!("Failed creating base download directory: {}", e);
    }
    let db = Database::new(&cfg.storage.turso_db_path)?;

    // 3. Initialize Email Sender
    let email = EmailSender::new(cfg.smtp.clone());

    // 4. Configure Tor Connection & ZLibraryClient
    let mode_str = cli.mode.unwrap_or_else(|| cfg.tor.mode.clone());
    let proxy_url = cli.proxy.unwrap_or_else(|| cfg.tor.proxy_url.clone());

    let mut tor_config = TorConfig::new(&proxy_url, &cfg.tor.onion_address);
    tor_config.mode = TorMode::from(mode_str.as_str());
    tor_config.fallback_onion_addresses = cfg.tor.fallback_onion_addresses.clone();

    let userid = cfg.auth.remix_userid.clone();
    let userkey = cfg.auth.remix_userkey.clone();

    let client = if let (Some(uid), Some(ukey)) = (userid, userkey) {
        ZLibraryClient::with_session(tor_config, uid, ukey)?
    } else {
        ZLibraryClient::new(tor_config)?
    };

    // 5. Build Shared AppState
    let state = AppState {
        config: cfg,
        client: Arc::new(Mutex::new(client)),
        db,
        email,
        pending_custom_emails: Arc::new(Mutex::new(std::collections::HashMap::new())),
    };

    // 6. Spawn Background Z-Library Auto-Login Task (Concurrently with Telegram Bot Startup)
    let bg_state = state.clone();
    tokio::spawn(async move {
        if let (Some(ref email_str), Some(ref pass_str)) = (&bg_state.config.auth.email, &bg_state.config.auth.password) {
            if !email_str.is_empty() && !pass_str.is_empty() {
                info!("Background Z-Library auto-login task starting over Tor for {}...", email_str);
                let mut client = bg_state.client.lock().await;
                match client.login(email_str, pass_str).await {
                    Ok(profile) => {
                        info!("Z-Library Authentication Successful! User: '{:?}'", profile.name);
                    }
                    Err(e) => {
                        warn!("Background Z-Library login warning (will auto-retry on search/doctor): {}", e);
                    }
                }
            }
        }
    });

    // 7. Spawn Tor Keep-Alive Background Task with Random Jitter (600 ± 300 seconds -> 300s to 900s)
    let keepalive_state = state.clone();
    tokio::spawn(async move {
        use rand::Rng;
        loop {
            let jitter_secs = rand::thread_rng().gen_range(300..=900);
            info!("Tor keep-alive next health check scheduled in {} seconds (jitter: 600 ± 300s)", jitter_secs);
            tokio::time::sleep(std::time::Duration::from_secs(jitter_secs)).await;

            info!("Running Tor connection keep-alive check...");

            let client = keepalive_state.client.lock().await.clone();
            match client.get_profile().await {
                Ok(profile) => {
                    info!(
                        "Tor keep-alive check successful! Active profile: '{:?}', Downloads today: {}/{}",
                        profile.name,
                        profile.downloads_today.unwrap_or(0),
                        profile.downloads_limit.unwrap_or(0)
                    );
                }
                Err(err) => {
                    warn!("Tor keep-alive check warning: {}. Attempting re-authentication...", err);
                    if let (Some(ref email_str), Some(ref pass_str)) = (&keepalive_state.config.auth.email, &keepalive_state.config.auth.password) {
                        if !email_str.is_empty() && !pass_str.is_empty() {
                            let mut client_mut = keepalive_state.client.lock().await;
                            if let Err(login_err) = client_mut.login(email_str, pass_str).await {
                                warn!("Tor keep-alive re-login failed: {}", login_err);
                            } else {
                                info!("Tor keep-alive re-login succeeded!");
                            }
                        }
                    }
                }
            }
        }
    });

    // 8. Spawn Calibre Content Server HTTP Task (if enabled)
    if state.config.calibre.enabled {
        let db_clone = state.db.clone();
        let host = state.config.calibre.host.clone();
        let port = cli.calibre_port.unwrap_or(state.config.calibre.port);
        let server_name = state.config.calibre.server_name.clone();

        tokio::spawn(async move {
            if let Err(e) = start_calibre_server(db_clone, &host, port, &server_name).await {
                warn!("Calibre Content Server task exited with error: {}", e);
            }
        });
    }

    // 9. Spawn Admin Web Dashboard HTTP Task (if enabled)
    if state.config.dashboard.enabled {
        let db_clone = state.db.clone();
        let cfg_clone = state.config.clone();
        let host = state.config.dashboard.host.clone();
        let port = cli.dashboard_port.unwrap_or(state.config.dashboard.port);
        let server_name = state.config.dashboard.server_name.clone();

        tokio::spawn(async move {
            if let Err(e) = start_dashboard_server(db_clone, cfg_clone, &host, port, &server_name).await {
                warn!("Admin Web Dashboard task exited with error: {}", e);
            }
        });
    }

    // 10. Start Telegram Bot Listener Daemon Immediately
    info!("Librero Daemon is active. Telegram Bot is online and listening for orders.");
    start_bot(state).await?;

    Ok(())
}


