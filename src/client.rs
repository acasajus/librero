use crate::models::{
    Book, LoginResponseData, SearchQuery, SessionTokens, UserProfile,
};
use crate::tor::TorConfig;
use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use std::collections::HashMap;
use tracing::{debug, error, info, warn};

/// Z-Library client over Tor network
#[derive(Debug)]
pub struct ZLibraryClient {
    pub config: TorConfig,
    client: Client,
    session: Option<SessionTokens>,
}

impl ZLibraryClient {
    /// Create a new ZLibraryClient with given Tor configuration
    pub fn new(config: TorConfig) -> Result<Self> {
        info!("Initializing ZLibraryClient with Tor configuration: mode={:?}, proxy={}", config.mode, config.proxy_url);
        let client = config.build_client()?;
        Ok(Self {
            config,
            client,
            session: None,
        })
    }

    /// Create client with existing session tokens
    pub fn with_session(config: TorConfig, userid: String, userkey: String) -> Result<Self> {
        info!("Initializing ZLibraryClient with active session (remix_userid={})", userid);
        let client = config.build_client()?;
        Ok(Self {
            config,
            client,
            session: Some(SessionTokens {
                remix_userid: userid,
                remix_userkey: userkey,
            }),
        })
    }

    /// Set session tokens directly
    pub fn set_session(&mut self, userid: String, userkey: String) {
        debug!("Updating session tokens: remix_userid={}", userid);
        self.session = Some(SessionTokens {
            remix_userid: userid,
            remix_userkey: userkey,
        });
    }

    /// Get current active session tokens if available
    pub fn session(&self) -> Option<&SessionTokens> {
        self.session.as_ref()
    }

    /// Build base URL endpoint path for a specific onion base host
    fn build_url(base: &str, path: &str) -> String {
        format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
    }

    /// Attach session headers (`remix-userid` & `remix-userkey`) to a request builder
    fn apply_session_headers(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(ref session) = self.session {
            debug!("Attaching session headers and cookies for user {}", session.remix_userid);
            req = req
                .header("remix-userid", &session.remix_userid)
                .header("remix-userkey", &session.remix_userkey)
                .header("Cookie", format!("remix_userid={}; remix_userkey={}", session.remix_userid, session.remix_userkey));
        } else {
            debug!("Sending unauthenticated request (no session tokens set)");
        }
        req
    }

    /// Authenticate against Z-Library using email & password
    pub async fn login(&mut self, email: &str, password: &str) -> Result<UserProfile> {
        info!("Attempting login for email '{}' over Tor hidden service...", email);
        let mut last_err = anyhow!("No onion address candidates configured");

        let candidates = self.config.onion_candidates();
        for (idx, base_url) in candidates.iter().enumerate() {
            let url = Self::build_url(base_url, "eapi/user/login");
            info!("[Candidate {}/{}] POST {}", idx + 1, candidates.len(), url);

            let mut params = HashMap::new();
            params.insert("email", email);
            params.insert("password", password);

            match self.client.post(&url).form(&params).send().await {
                Ok(res) => {
                    let status = res.status();
                    debug!("Received HTTP response status: {} from {}", status, url);

                    if !status.is_success() {
                        last_err = anyhow!("Login request failed at {} with HTTP status: {}", url, status);
                        warn!("{}", last_err);
                        continue;
                    }

                    let text = match res.text().await {
                        Ok(t) => t,
                        Err(e) => {
                            last_err = anyhow!("Failed to read response body from {}: {}", url, e);
                            error!("{}", last_err);
                            continue;
                        }
                    };

                    debug!("Raw login response body (length: {} bytes): {}", text.len(), text);

                    let json_val: serde_json::Value = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(e) => {
                            last_err = anyhow!("Failed to parse JSON from {}: {}", url, e);
                            error!("{}", last_err);
                            continue;
                        }
                    };

                    if json_val.get("success").and_then(|v| v.as_u64()).unwrap_or(0) == 0 
                        && json_val.get("status").and_then(|v| v.as_u64()).unwrap_or(0) == 0 {
                        let msg = json_val.get("message").and_then(|v| v.as_str()).unwrap_or("Authentication failed");
                        error!("Login failed with API error message: {}", msg);
                        return Err(anyhow!("Z-Library Login Failed: {}", msg));
                    }

                    let response_data: LoginResponseData = serde_json::from_value(
                        json_val.get("response").cloned().unwrap_or(json_val.clone())
                    ).context("Failed to extract login user data")?;

                    let user = response_data.user.ok_or_else(|| anyhow!("No user object in login response"))?;

                    let userid = user.remix_userid.clone()
                        .or(response_data.remix_userid)
                        .or_else(|| user.id.map(|i| i.to_string()))
                        .ok_or_else(|| anyhow!("remix_userid missing in login response"))?;

                    let userkey = user.remix_userkey.clone()
                        .or(response_data.remix_userkey)
                        .ok_or_else(|| anyhow!("remix_userkey missing in login response"))?;

                    info!("Successfully logged in as '{}' (User ID: {}, remix_userid={})", user.name.as_deref().unwrap_or(email), user.id.unwrap_or(0), userid);

                    self.session = Some(SessionTokens {
                        remix_userid: userid,
                        remix_userkey: userkey,
                    });

                    return Ok(user);
                }
                Err(err) => {
                    last_err = anyhow!("Failed connecting to onion mirror {}: {}", base_url, err);
                    warn!("{}", last_err);
                }
            }
        }

        error!("Login failed on all onion mirror candidates");
        Err(last_err)
    }

    /// Retrieve user profile and remaining download limits
    pub async fn get_profile(&self) -> Result<UserProfile> {
        info!("Retrieving user profile and download limits...");
        let mut last_err = anyhow!("No onion candidates configured");

        let candidates = self.config.onion_candidates();
        for (idx, base_url) in candidates.iter().enumerate() {
            let url = Self::build_url(base_url, "eapi/user/profile");
            debug!("[Candidate {}/{}] POST {}", idx + 1, candidates.len(), url);

            let req = self.client.post(&url);
            let req = self.apply_session_headers(req);

            match req.send().await {
                Ok(res) => {
                    debug!("Profile response HTTP status: {}", res.status());
                    let text = res.text().await?;
                    debug!("Raw profile JSON payload: {}", text);

                    let json_val: serde_json::Value = serde_json::from_str(&text)?;
                    let user: UserProfile = serde_json::from_value(
                        json_val.get("user").cloned()
                            .or_else(|| json_val.get("response").and_then(|r| r.get("user")).cloned())
                            .unwrap_or(json_val)
                    ).context("Failed to parse user profile")?;

                    info!("Profile retrieved: User '{:?}', Downloads Today: {}/{}", 
                        user.name, 
                        user.downloads_today.unwrap_or(0), 
                        user.downloads_limit.unwrap_or(0)
                    );
                    return Ok(user);
                }
                Err(e) => {
                    last_err = anyhow!("Failed to query profile on {}: {}", base_url, e);
                    warn!("{}", last_err);
                }
            }
        }

        Err(last_err)
    }

    /// Search books on Z-Library
    pub async fn search(&self, query: &SearchQuery) -> Result<Vec<Book>> {
        info!("Executing book search over Tor: query='{}', page={}, limit={}, extension={:?}", query.query, query.page, query.limit, query.extension);
        let mut last_err = anyhow!("No onion candidates configured");

        let candidates = self.config.onion_candidates();
        for (idx, base_url) in candidates.iter().enumerate() {
            let url = Self::build_url(base_url, "eapi/book/search");
            debug!("[Candidate {}/{}] POST {}", idx + 1, candidates.len(), url);

            let mut params = HashMap::new();
            params.insert("message", query.query.clone());
            params.insert("page", query.page.to_string());
            params.insert("limit", query.limit.to_string());

            if let Some(ref from) = query.year_from {
                params.insert("yearFrom", from.to_string());
            }
            if let Some(ref to) = query.year_to {
                params.insert("yearTo", to.to_string());
            }
            if let Some(ref lang) = query.language {
                params.insert("languages", lang.clone());
            }
            if let Some(ref ext) = query.extension {
                params.insert("extensions", ext.clone());
            }

            let req = self.client.post(&url).form(&params);
            let req = self.apply_session_headers(req);

            match req.send().await {
                Ok(res) => {
                    debug!("Search HTTP response status: {}", res.status());
                    let text = res.text().await.context("Failed to read search response")?;
                    debug!("Raw search response JSON length: {} bytes", text.len());

                    let json_val: serde_json::Value = serde_json::from_str(&text)
                        .context(format!("Failed to parse search JSON: {}", text))?;

                    let books_val = json_val.get("books")
                        .or_else(|| json_val.get("response").and_then(|r| r.get("books")))
                        .ok_or_else(|| anyhow!("No 'books' array in response"))?;

                    let books: Vec<Book> = serde_json::from_value(books_val.clone())
                        .context("Failed to deserialize books list")?;

                    info!("Successfully found {} books for query '{}'", books.len(), query.query);
                    return Ok(books);
                }
                Err(e) => {
                    last_err = anyhow!("Failed search on onion mirror {}: {}", base_url, e);
                    warn!("{}", last_err);
                }
            }
        }

        Err(last_err)
    }

    /// Get direct download URL for a given book
    pub async fn get_download_url(&self, book_id: u64, book_hash: &str) -> Result<String> {
        info!("Fetching direct download link for book ID {} (hash: {})...", book_id, book_hash);
        let mut last_err = anyhow!("No onion candidates configured");

        let candidates = self.config.onion_candidates();
        for (idx, base_url) in candidates.iter().enumerate() {
            let path = format!("eapi/book/{}/{}/file", book_id, book_hash);
            let url = Self::build_url(base_url, &path);
            debug!("[Candidate {}/{}] GET {}", idx + 1, candidates.len(), url);

            let req = self.client.get(&url);
            let req = self.apply_session_headers(req);

            match req.send().await {
                Ok(res) => {
                    debug!("Download link HTTP response status: {}", res.status());
                    let text = res.text().await?;
                    debug!("Raw download link JSON response: {}", text);

                    let json_val: serde_json::Value = serde_json::from_str(&text)?;

                    let dl_url = json_val.get("file").and_then(|f| f.get("url")).and_then(|u| u.as_str())
                        .or_else(|| json_val.get("url").and_then(|u| u.as_str()))
                        .or_else(|| json_val.get("response").and_then(|r| r.get("url")).and_then(|u| u.as_str()))
                        .ok_or_else(|| anyhow!("Download URL not found in response: {}", text))?;

                    info!("Resolved download URL for book {}: {}", book_id, dl_url);
                    return Ok(dl_url.to_string());
                }
                Err(e) => {
                    last_err = anyhow!("Failed fetching download link from {}: {}", base_url, e);
                    warn!("{}", last_err);
                }
            }
        }

        Err(last_err)
    }

    /// Download book binary bytes from the given URL over Tor
    pub async fn download_book_bytes(&self, download_url: &str) -> Result<Vec<u8>> {
        let full_url = if download_url.starts_with("http") {
            download_url.to_string()
        } else {
            Self::build_url(&self.config.onion_address, download_url)
        };

        info!("Downloading book binary file over Tor stream from: {}", full_url);
        let req = self.client.get(&full_url);
        let req = self.apply_session_headers(req);

        let res = req.send().await.context("Failed to download book content")?;
        let status = res.status();
        debug!("Book download HTTP status: {}", status);

        if !status.is_success() {
            error!("Failed downloading book file. HTTP Status: {}", status);
            return Err(anyhow!("Failed downloading book, HTTP status: {}", status));
        }

        let bytes = res.bytes().await.context("Failed to read book file stream")?;
        info!("Download complete: received {} bytes", bytes.len());
        Ok(bytes.to_vec())
    }
}
