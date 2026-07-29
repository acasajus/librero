use crate::models::{
    Book, LoginResponseData, SearchQuery, SessionTokens, UserProfile,
};
use crate::tor::TorConfig;
use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

const MAX_RETRIES_PER_MIRROR: u32 = 3;
const RETRY_BACKOFF_MS: u64 = 1500;

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

    /// Check if an error message indicates a retryable Tor / onion network issue
    fn is_retryable_onion_error(err_msg: &str) -> bool {
        let lower = err_msg.to_lowercase();
        lower.contains("timeout")
            || lower.contains("timed out")
            || lower.contains("connection refused")
            || lower.contains("reset")
            || lower.contains("broken pipe")
            || lower.contains("onion")
            || lower.contains("socks")
            || lower.contains("circuit")
            || lower.contains("unreachable")
            || lower.contains("502")
            || lower.contains("503")
            || lower.contains("504")
            || lower.contains("eof")
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

            for attempt in 1..=MAX_RETRIES_PER_MIRROR {
                info!("[Candidate {}/{} | Attempt {}/{}] POST {}", idx + 1, candidates.len(), attempt, MAX_RETRIES_PER_MIRROR, url);

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
                            if status.is_server_error() && attempt < MAX_RETRIES_PER_MIRROR {
                                sleep(Duration::from_millis(RETRY_BACKOFF_MS)).await;
                                continue;
                            }
                            break;
                        }

                        let text = match res.text().await {
                            Ok(t) => t,
                            Err(e) => {
                                last_err = anyhow!("Failed to read response body from {}: {}", url, e);
                                error!("{}", last_err);
                                break;
                            }
                        };

                        let json_val: serde_json::Value = match serde_json::from_str(&text) {
                            Ok(v) => v,
                            Err(e) => {
                                last_err = anyhow!("Failed to parse JSON from {}: {}", url, e);
                                error!("{}", last_err);
                                break;
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
                        let err_msg = err.to_string();
                        last_err = anyhow!("Failed connecting to onion mirror {}: {}", base_url, err_msg);

                        if Self::is_retryable_onion_error(&err_msg) && attempt < MAX_RETRIES_PER_MIRROR {
                            warn!("Onion circuit/network error on {} (Attempt {}/{}): {}. Retrying in {}ms...", base_url, attempt, MAX_RETRIES_PER_MIRROR, err_msg, RETRY_BACKOFF_MS);
                            sleep(Duration::from_millis(RETRY_BACKOFF_MS)).await;
                        } else {
                            warn!("{}", last_err);
                            break;
                        }
                    }
                }
            }
        }

        error!("Login failed on all onion mirror candidates after retries");
        Err(last_err)
    }

    /// Retrieve user profile and remaining download limits
    pub async fn get_profile(&self) -> Result<UserProfile> {
        info!("Retrieving user profile and download limits...");
        let mut last_err = anyhow!("No onion candidates configured");

        let candidates = self.config.onion_candidates();
        for (idx, base_url) in candidates.iter().enumerate() {
            let url = Self::build_url(base_url, "eapi/user/profile");

            for attempt in 1..=MAX_RETRIES_PER_MIRROR {
                debug!("[Candidate {}/{} | Attempt {}/{}] POST {}", idx + 1, candidates.len(), attempt, MAX_RETRIES_PER_MIRROR, url);

                let req = self.client.post(&url);
                let req = self.apply_session_headers(req);

                match req.send().await {
                    Ok(res) => {
                        let status = res.status();
                        if !status.is_success() && status.is_server_error() && attempt < MAX_RETRIES_PER_MIRROR {
                            sleep(Duration::from_millis(RETRY_BACKOFF_MS)).await;
                            continue;
                        }

                        let text = res.text().await?;
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
                        let err_msg = e.to_string();
                        last_err = anyhow!("Failed to query profile on {}: {}", base_url, err_msg);

                        if Self::is_retryable_onion_error(&err_msg) && attempt < MAX_RETRIES_PER_MIRROR {
                            warn!("Onion error querying profile on {} (Attempt {}/{}): {}. Retrying...", base_url, attempt, MAX_RETRIES_PER_MIRROR, err_msg);
                            sleep(Duration::from_millis(RETRY_BACKOFF_MS)).await;
                        } else {
                            warn!("{}", last_err);
                            break;
                        }
                    }
                }
            }
        }

        Err(last_err)
    }

    /// Search books on Z-Library with automatic retries for Tor/Onion errors
    pub async fn search(&self, query: &SearchQuery) -> Result<Vec<Book>> {
        info!("Executing book search over Tor: query='{}', page={}, limit={}, extension={:?}", query.query, query.page, query.limit, query.extension);
        let mut last_err = anyhow!("No onion candidates configured");

        let candidates = self.config.onion_candidates();
        for (idx, base_url) in candidates.iter().enumerate() {
            let url = Self::build_url(base_url, "eapi/book/search");

            for attempt in 1..=MAX_RETRIES_PER_MIRROR {
                info!("[Candidate {}/{} | Attempt {}/{}] Searching '{}' at {}", idx + 1, candidates.len(), attempt, MAX_RETRIES_PER_MIRROR, query.query, url);

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
                        let status = res.status();
                        debug!("Search HTTP response status: {}", status);

                        if !status.is_success() && status.is_server_error() && attempt < MAX_RETRIES_PER_MIRROR {
                            warn!("Server error HTTP {} searching on {}. Retrying attempt {}/{}...", status, base_url, attempt + 1, MAX_RETRIES_PER_MIRROR);
                            sleep(Duration::from_millis(RETRY_BACKOFF_MS)).await;
                            continue;
                        }

                        let text = res.text().await.context("Failed to read search response")?;
                        debug!("Raw search response JSON length: {} bytes", text.len());

                        let json_val: serde_json::Value = serde_json::from_str(&text)
                            .context(format!("Failed to parse search JSON: {}", text))?;

                        let books_val = json_val.get("books")
                            .or_else(|| json_val.get("response").and_then(|r| r.get("books")))
                            .ok_or_else(|| anyhow!("No 'books' array in response. Full JSON response was: {}", text))?;

                        let books_arr = books_val.as_array()
                            .ok_or_else(|| anyhow!("'books' field is not an array"))?;

                        let mut books = Vec::new();
                        for b_val in books_arr {
                            if let Some(book) = Book::from_json_value(b_val) {
                                books.push(book);
                            } else {
                                warn!("Skipped unparsable book object: {:?}", b_val);
                            }
                        }

                        info!("Successfully found {} books for query '{}'", books.len(), query.query);
                        return Ok(books);
                    }
                    Err(e) => {
                        let err_msg = e.to_string();
                        last_err = anyhow!("Failed search on onion mirror {}: {}", base_url, err_msg);

                        if Self::is_retryable_onion_error(&err_msg) && attempt < MAX_RETRIES_PER_MIRROR {
                            warn!("Onion error during search on {} (Attempt {}/{}): {}. Retrying in {}ms...", base_url, attempt, MAX_RETRIES_PER_MIRROR, err_msg, RETRY_BACKOFF_MS);
                            sleep(Duration::from_millis(RETRY_BACKOFF_MS)).await;
                        } else {
                            warn!("{}", last_err);
                            break;
                        }
                    }
                }
            }
        }

        Err(last_err)
    }

    /// Get direct download URL for a given book with retries
    pub async fn get_download_url(&self, book_id: u64, book_hash: &str) -> Result<String> {
        info!("Fetching direct download link for book ID {} (hash: {})...", book_id, book_hash);
        let mut last_err = anyhow!("No onion candidates configured");

        let candidates = self.config.onion_candidates();
        for (idx, base_url) in candidates.iter().enumerate() {
            let path = format!("eapi/book/{}/{}/file", book_id, book_hash);
            let url = Self::build_url(base_url, &path);

            for attempt in 1..=MAX_RETRIES_PER_MIRROR {
                debug!("[Candidate {}/{} | Attempt {}/{}] GET {}", idx + 1, candidates.len(), attempt, MAX_RETRIES_PER_MIRROR, url);

                let req = self.client.get(&url);
                let req = self.apply_session_headers(req);

                match req.send().await {
                    Ok(res) => {
                        let status = res.status();
                        if !status.is_success() && status.is_server_error() && attempt < MAX_RETRIES_PER_MIRROR {
                            sleep(Duration::from_millis(RETRY_BACKOFF_MS)).await;
                            continue;
                        }

                        let text = res.text().await?;
                        let json_val: serde_json::Value = serde_json::from_str(&text)?;

                        let dl_url = json_val.get("file").and_then(|f| f.get("downloadLink").or_else(|| f.get("url")).or_else(|| f.get("dlUrl"))).and_then(|u| u.as_str())
                            .or_else(|| json_val.get("response").and_then(|r| r.get("file")).and_then(|f| f.get("downloadLink").or_else(|| f.get("url"))).and_then(|u| u.as_str()))
                            .or_else(|| json_val.get("response").and_then(|r| r.get("url").or_else(|| r.get("downloadLink"))).and_then(|u| u.as_str()))
                            .or_else(|| json_val.get("downloadLink").and_then(|u| u.as_str()))
                            .or_else(|| json_val.get("url").and_then(|u| u.as_str()))
                            .ok_or_else(|| anyhow!("Download URL not found in response: {}", text))?;

                        info!("Resolved download URL for book {}: {}", book_id, dl_url);
                        return Ok(dl_url.to_string());
                    }
                    Err(e) => {
                        let err_msg = e.to_string();
                        last_err = anyhow!("Failed fetching download link from {}: {}", base_url, err_msg);

                        if Self::is_retryable_onion_error(&err_msg) && attempt < MAX_RETRIES_PER_MIRROR {
                            warn!("Onion error resolving download link on {} (Attempt {}/{}): {}. Retrying...", base_url, attempt, MAX_RETRIES_PER_MIRROR, err_msg);
                            sleep(Duration::from_millis(RETRY_BACKOFF_MS)).await;
                        } else {
                            warn!("{}", last_err);
                            break;
                        }
                    }
                }
            }
        }

        Err(last_err)
    }

    /// Download book binary bytes from the given URL over Tor with retries across onion candidates
    pub async fn download_book_bytes(&self, download_url: &str) -> Result<Vec<u8>> {
        info!("Downloading book binary file over Tor stream for URL: {}", download_url);

        // If download_url is an absolute HTTP/HTTPS URL (e.g. https://dln1.ncdn.ec/books-files/...), download directly over Tor proxy
        if download_url.starts_with("http://") || download_url.starts_with("https://") {
            let mut last_err = anyhow!("Download failed from {}", download_url);
            for attempt in 1..=MAX_RETRIES_PER_MIRROR {
                info!("[Attempt {}/{}] Downloading book file directly from: {}", attempt, MAX_RETRIES_PER_MIRROR, download_url);

                let req = self.client.get(download_url);
                let req = self.apply_session_headers(req);

                match req.send().await {
                    Ok(res) => {
                        let status = res.status();
                        debug!("Book download HTTP status: {} from {}", status, download_url);

                        if !status.is_success() {
                            last_err = anyhow!("Failed downloading book from {}, HTTP status: {}", download_url, status);
                            if status.is_server_error() && attempt < MAX_RETRIES_PER_MIRROR {
                                warn!("HTTP {} downloading book. Retrying attempt {}/{}...", status, attempt + 1, MAX_RETRIES_PER_MIRROR);
                                sleep(Duration::from_millis(RETRY_BACKOFF_MS)).await;
                                continue;
                            }
                            return Err(last_err);
                        }

                        let bytes = res.bytes().await.context("Failed to read book file stream")?;
                        info!("Download complete: received {} bytes", bytes.len());
                        return Ok(bytes.to_vec());
                    }
                    Err(e) => {
                        let err_msg = e.to_string();
                        last_err = anyhow!("Failed downloading book stream from {}: {}", download_url, err_msg);

                        if Self::is_retryable_onion_error(&err_msg) && attempt < MAX_RETRIES_PER_MIRROR {
                            warn!("Network/Tor error downloading book (Attempt {}/{}): {}. Retrying in {}ms...", attempt, MAX_RETRIES_PER_MIRROR, err_msg, RETRY_BACKOFF_MS);
                            sleep(Duration::from_millis(RETRY_BACKOFF_MS)).await;
                        } else {
                            warn!("{}", last_err);
                            break;
                        }
                    }
                }
            }
            return Err(last_err);
        }

        // Relative path -> attempt download across onion mirror candidates
        let mut last_err = anyhow!("Download failed on all onion mirrors");
        let candidates = self.config.onion_candidates();
        for (idx, base_url) in candidates.iter().enumerate() {
            let full_url = Self::build_url(base_url, download_url);

            for attempt in 1..=MAX_RETRIES_PER_MIRROR {
                info!("[Candidate {}/{} | Attempt {}/{}] Downloading book from {}", idx + 1, candidates.len(), attempt, MAX_RETRIES_PER_MIRROR, full_url);

                let req = self.client.get(&full_url);
                let req = self.apply_session_headers(req);

                match req.send().await {
                    Ok(res) => {
                        let status = res.status();
                        debug!("Book download HTTP status: {} from {}", status, full_url);

                        if !status.is_success() {
                            last_err = anyhow!("Failed downloading book from {}, HTTP status: {}", full_url, status);
                            if status.is_server_error() && attempt < MAX_RETRIES_PER_MIRROR {
                                warn!("HTTP {} downloading book from {}. Retrying attempt {}/{}...", status, base_url, attempt + 1, MAX_RETRIES_PER_MIRROR);
                                sleep(Duration::from_millis(RETRY_BACKOFF_MS)).await;
                                continue;
                            }
                            break;
                        }

                        let bytes = res.bytes().await.context("Failed to read book file stream")?;
                        info!("Download complete from {}: received {} bytes", base_url, bytes.len());
                        return Ok(bytes.to_vec());
                    }
                    Err(e) => {
                        let err_msg = e.to_string();
                        last_err = anyhow!("Failed downloading book stream from {}: {}", base_url, err_msg);

                        if Self::is_retryable_onion_error(&err_msg) && attempt < MAX_RETRIES_PER_MIRROR {
                            warn!("Onion error downloading book from {} (Attempt {}/{}): {}. Retrying in {}ms...", base_url, attempt, MAX_RETRIES_PER_MIRROR, err_msg, RETRY_BACKOFF_MS);
                            sleep(Duration::from_millis(RETRY_BACKOFF_MS)).await;
                        } else {
                            warn!("{}", last_err);
                            break;
                        }
                    }
                }
            }
        }

        Err(last_err)
    }
}
