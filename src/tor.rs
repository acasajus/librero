use reqwest::{Client, Proxy};
use std::net::{SocketAddr, TcpListener as StdTcpListener, TcpStream as StdTcpStream};
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{error, info, warn};

#[cfg(feature = "embedded-tor")]
use arti_client::{TorClient, TorClientConfig};
#[cfg(feature = "embedded-tor")]
use tor_rtcompat::PreferredRuntime;

/// Default primary Z-Library Tor .onion address
pub const DEFAULT_ZLIB_ONION: &str = "http://loginzlib2vrak5zzpcocc3ouizykn6k5qecgj2tzlnab5wcbqhembyd.onion";

/// Default fallback Z-Library Tor .onion address
pub const FALLBACK_ZLIB_ONION: &str = "http://bookszlibb74ugqojhzhg2a63w5i2atv5bqarulgczawnbmsb6s6qead.onion";

/// Default local Tor SOCKS5 proxy address
pub const DEFAULT_SOCKS5_PROXY: &str = "socks5h://127.0.0.1:9050";

#[derive(Error, Debug)]
pub enum TorError {
    #[error("Failed to build HTTP client with Tor proxy: {0}")]
    ClientBuildError(#[from] reqwest::Error),
    #[error("Invalid proxy URL: {0}")]
    InvalidProxyUrl(String),
    #[error("Embedded Arti Tor client bootstrap error: {0}")]
    EmbeddedTorError(String),
}

/// Tor Connection Mode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TorMode {
    /// Pure Rust embedded Tor client (Arti) connecting to public Tor nodes
    Embedded,
    /// Local SOCKS5 Tor daemon or Tor Browser proxy
    Socks5,
    /// Auto: Try local proxy first, fall back to embedded Arti Tor
    Auto,
}

impl From<&str> for TorMode {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "socks" | "socks5" => TorMode::Socks5,
            "auto" => TorMode::Auto,
            _ => TorMode::Embedded,
        }
    }
}

/// Configuration for connecting to the Tor network with Z-Library mirror fallbacks.
#[derive(Debug, Clone)]
pub struct TorConfig {
    /// Tor Connection Mode
    pub mode: TorMode,

    /// The SOCKS5 proxy URL
    pub proxy_url: String,

    /// Primary Z-Library Onion address
    pub onion_address: String,

    /// List of fallback Z-Library Onion addresses if primary mirror is down
    pub fallback_onion_addresses: Vec<String>,

    /// Request timeout in seconds
    pub timeout: Duration,

    /// Custom User-Agent header string
    pub user_agent: String,
}

impl Default for TorConfig {
    fn default() -> Self {
        Self {
            mode: TorMode::Embedded,
            proxy_url: DEFAULT_SOCKS5_PROXY.to_string(),
            onion_address: DEFAULT_ZLIB_ONION.to_string(),
            fallback_onion_addresses: vec![FALLBACK_ZLIB_ONION.to_string()],
            timeout: Duration::from_secs(45),
            user_agent: "Mozilla/5.0 (Android 13; Mobile; rv:109.0) Gecko/114.0 Firefox/114.0".to_string(),
        }
    }
}

impl TorConfig {
    pub fn new(proxy_url: impl Into<String>, onion_address: impl Into<String>) -> Self {
        Self {
            proxy_url: proxy_url.into(),
            onion_address: onion_address.into(),
            fallback_onion_addresses: vec![FALLBACK_ZLIB_ONION.to_string()],
            ..Default::default()
        }
    }

    /// Check if a local TCP proxy port is accepting connections
    pub fn check_tcp_port(addr_str: &str) -> bool {
        let clean = addr_str
            .trim_start_matches("socks5h://")
            .trim_start_matches("socks5://")
            .trim_start_matches("http://");

        if let Ok(socket_addr) = clean.parse::<SocketAddr>() {
            StdTcpStream::connect_timeout(&socket_addr, Duration::from_millis(150)).is_ok()
        } else {
            false
        }
    }

    /// Build a reqwest HTTP client configured with embedded Arti Tor or local SOCKS5 proxy
    pub fn build_client(&self) -> Result<Client, TorError> {
        let proxy_target = match self.mode {
            TorMode::Socks5 => {
                info!("Using configured SOCKS5 proxy: {}", self.proxy_url);
                self.proxy_url.clone()
            }
            TorMode::Auto => {
                if Self::check_tcp_port(&self.proxy_url) {
                    info!("Detected local Tor daemon at {}", self.proxy_url);
                    self.proxy_url.clone()
                } else if Self::check_tcp_port("127.0.0.1:9150") {
                    let proxy = "socks5h://127.0.0.1:9150".to_string();
                    info!("Detected local Tor Browser at {}", proxy);
                    proxy
                } else {
                    info!("No local Tor daemon detected. Initializing embedded Arti Tor client...");
                    self.start_embedded_arti()?
                }
            }
            TorMode::Embedded => {
                info!("Initializing embedded Arti Tor client (connecting directly to public Tor nodes)...");
                self.start_embedded_arti()?
            }
        };

        info!("Configuring SOCKS5 HTTP client via {}", proxy_target);
        let proxy = Proxy::all(&proxy_target)
            .map_err(|_| TorError::InvalidProxyUrl(proxy_target.clone()))?;

        let client = Client::builder()
            .proxy(proxy)
            .timeout(self.timeout)
            .user_agent(&self.user_agent)
            .cookie_store(true)
            .build()?;

        Ok(client)
    }

    /// Start embedded Arti Tor client & SOCKS5 proxy bridge
    fn start_embedded_arti(&self) -> Result<String, TorError> {
        #[cfg(feature = "embedded-tor")]
        {
            // Find an available random local loopback port
            let std_listener = StdTcpListener::bind("127.0.0.1:0")
                .map_err(|e| TorError::EmbeddedTorError(format!("Failed to bind loopback listener: {}", e)))?;
            let local_addr = std_listener.local_addr()
                .map_err(|e| TorError::EmbeddedTorError(format!("Failed to get local port: {}", e)))?;

            std_listener.set_nonblocking(true)
                .map_err(|e| TorError::EmbeddedTorError(format!("Nonblocking error: {}", e)))?;

            let proxy_url = format!("socks5h://{}", local_addr);
            info!("Bootstrapping Arti Tor client & starting embedded SOCKS5 proxy at {}...", proxy_url);

            tokio::spawn(async move {
                if let Err(e) = run_arti_socks_server(std_listener).await {
                    error!("Embedded Arti Tor proxy error: {}", e);
                }
            });

            Ok(proxy_url)
        }
        #[cfg(not(feature = "embedded-tor"))]
        {
            Err(TorError::EmbeddedTorError("embedded-tor feature not enabled".to_string()))
        }
    }

    /// Get all target onion base URLs to attempt in order (primary followed by fallbacks)
    pub fn onion_candidates(&self) -> Vec<String> {
        let mut candidates = vec![self.onion_address.clone()];
        for f in &self.fallback_onion_addresses {
            if !candidates.contains(f) {
                candidates.push(f.clone());
            }
        }
        candidates
    }
}

/// Run embedded SOCKS5 proxy server using Arti TorClient
#[cfg(feature = "embedded-tor")]
async fn run_arti_socks_server(std_listener: StdTcpListener) -> anyhow::Result<()> {
    let mut config_builder = TorClientConfig::builder();
    config_builder.address_filter().allow_onion_addrs(true);
    let config = config_builder.build()?;

    info!("Bootstrapping embedded Arti Tor circuit with public Tor relays...");
    let tor_client = TorClient::create_bootstrapped(config).await?;
    info!("Embedded Arti Tor client successfully bootstrapped to the Tor network!");

    let listener = TcpListener::from_std(std_listener)?;

    loop {
        let (mut socket, peer_addr) = listener.accept().await?;
        let tor_client = tor_client.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_socks5_connection(&mut socket, tor_client).await {
                warn!("SOCKS5 connection from {} failed: {}", peer_addr, e);
            }
        });
    }
}

/// Handle a SOCKS5 proxy request and bridge it to Arti Tor stream
#[cfg(feature = "embedded-tor")]
async fn handle_socks5_connection(
    socket: &mut tokio::net::TcpStream,
    tor_client: TorClient<PreferredRuntime>,
) -> anyhow::Result<()> {
    let mut buf = [0u8; 512];

    // 1. SOCKS5 Greeting Handshake
    socket.read_exact(&mut buf[0..2]).await?;
    let ver = buf[0];
    let nmethods = buf[1] as usize;
    if ver != 5 {
        return Err(anyhow::anyhow!("Unsupported SOCKS version: {}", ver));
    }

    let mut methods = vec![0u8; nmethods];
    socket.read_exact(&mut methods).await?;
    socket.write_all(&[0x05, 0x00]).await?; // No Auth

    // 2. SOCKS5 Request
    socket.read_exact(&mut buf[0..4]).await?;
    let cmd = buf[1];
    let atyp = buf[3];

    if cmd != 1 {
        return Err(anyhow::anyhow!("Unsupported SOCKS command: {}", cmd));
    }

    let target_host = match atyp {
        3 => {
            // Domain Name
            socket.read_exact(&mut buf[0..1]).await?;
            let len = buf[0] as usize;
            let mut domain_buf = vec![0u8; len];
            socket.read_exact(&mut domain_buf).await?;
            String::from_utf8(domain_buf)?
        }
        1 => {
            // IPv4
            let mut ip = [0u8; 4];
            socket.read_exact(&mut ip).await?;
            format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
        }
        _ => return Err(anyhow::anyhow!("Unsupported SOCKS address type: {}", atyp)),
    };

    let mut port_buf = [0u8; 2];
    socket.read_exact(&mut port_buf).await?;
    let port = u16::from_be_bytes(port_buf);

    info!("Routing SOCKS5 request for {}:{} via embedded Arti Tor...", target_host, port);

    // 3. Establish connection through Arti Tor Client
    match tor_client.connect((target_host.as_str(), port)).await {
        Ok(mut tor_stream) => {
            // Success response: [0x05, 0x00, 0x00, 0x01, 0,0,0,0, 0,0]
            socket.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await?;
            tokio::io::copy_bidirectional(socket, &mut tor_stream).await?;
        }
        Err(err) => {
            warn!("Failed to establish Arti Tor circuit for {}:{}: {}", target_host, port, err);
            // Host unreachable error response
            socket.write_all(&[0x05, 0x04, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await?;
        }
    }

    Ok(())
}
