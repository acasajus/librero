use serde::{Deserialize, Serialize};

/// Clean book titles by removing Z-Library domain watermarks (e.g. '(z-library.sk, 1lib.sk, z-lib.sk)'),
/// stripping dangling unclosed parentheses/brackets, and trimming trailing truncation symbols.
pub fn clean_book_title(raw_title: &str) -> String {
    let mut title = raw_title.trim().to_string();

    // 1. Remove parenthetical domain watermarks e.g. (z-library.sk, 1lib.sk, z-lib.sk) or [z-lib.org]
    while let Some(open_paren) = title.rfind('(').or_else(|| title.rfind('[')) {
        let suffix = &title[open_paren..];
        let suffix_lower = suffix.to_lowercase();
        if suffix_lower.contains("z-lib")
            || suffix_lower.contains("1lib")
            || suffix_lower.contains("b-ok")
            || suffix_lower.contains("libgen")
            || suffix_lower.contains(".sk")
            || suffix_lower.contains(".se")
            || suffix_lower.contains(".is")
            || suffix_lower.contains(".org")
            || suffix_lower.contains(".cc")
            || suffix_lower.contains(".gs")
            || suffix_lower.contains(".rs")
            || suffix_lower.contains(".site")
            || suffix_lower.contains(".to")
        {
            title = title[..open_paren].trim().to_string();
        } else {
            break;
        }
    }

    // 2. Remove dangling / unclosed '(' or '[' at the end of the title (e.g. 'The Stars My Destination (A')
    while let Some(open_idx) = title.rfind(|c| c == '(' || c == '[') {
        let open_char = title.as_bytes()[open_idx] as char;
        let close_char = if open_char == '(' { ')' } else { ']' };
        let rest = &title[open_idx..];
        if !rest.contains(close_char) {
            title = title[..open_idx].trim().to_string();
        } else {
            break;
        }
    }

    // 3. Trim trailing truncation markers like '_', '...', '-', or extra punctuation left at the end
    loop {
        let len_before = title.len();
        title = title.trim_end_matches(|c: char| c == '_' || c == '.' || c == '-' || c.is_whitespace()).to_string();
        if title.len() == len_before {
            break;
        }
    }

    if title.is_empty() {
        raw_title.trim().to_string()
    } else {
        title
    }
}


/// Represents the Z-Library credentials for authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credentials {

    pub email: String,
    pub password: String,
}

/// Active session tokens retrieved after successful login.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTokens {
    pub remix_userid: String,
    pub remix_userkey: String,
}

/// General API response wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    pub success: Option<u8>,
    pub status: Option<u8>,
    pub message: Option<String>,
    pub response: Option<T>,
}

/// User details returned from login/profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: Option<u64>,
    pub email: Option<String>,
    pub name: Option<String>,
    #[serde(alias = "downloadsToday")]
    pub downloads_today: Option<u32>,
    #[serde(alias = "downloadsLimit")]
    pub downloads_limit: Option<u32>,
    pub remix_userid: Option<String>,
    pub remix_userkey: Option<String>,
}

/// Login API Response structure from eAPI `/eapi/user/login`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponseData {
    pub user: Option<UserProfile>,
    #[serde(alias = "remix_userid")]
    pub remix_userid: Option<String>,
    #[serde(alias = "remix_userkey")]
    pub remix_userkey: Option<String>,
}

/// Book model representing a search result or detailed book info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Book {
    pub id: u64,
    pub title: String,
    pub author: Option<String>,
    pub publisher: Option<String>,
    pub year: Option<String>,
    pub language: Option<String>,
    pub extension: Option<String>,
    pub filesize: Option<u64>,
    pub filesize_string: Option<String>,
    pub cover: Option<String>,
    pub hash: Option<String>,
    pub description: Option<String>,
    pub rating: Option<String>,
    pub quality: Option<String>,
    #[serde(alias = "dlUrl")]
    pub download_url: Option<String>,
}

impl Book {
    /// Resilient JSON parser for Z-Library book objects (handles string/int type variations)
    pub fn from_json_value(val: &serde_json::Value) -> Option<Self> {
        let obj = val.as_object()?;

        // Parse ID: string or number
        let id = obj.get("id").and_then(|v| {
            if let Some(n) = v.as_u64() {
                Some(n)
            } else if let Some(s) = v.as_str() {
                s.parse::<u64>().ok()
            } else {
                None
            }
        })?;

        // Parse Title: string or printable number
        let raw_title = obj.get("title").and_then(|v| {
            if let Some(s) = v.as_str() {
                Some(s.to_string())
            } else if v.is_number() {
                Some(v.to_string())
            } else {
                None
            }
        }).unwrap_or_else(|| format!("Book {}", id));

        let title = clean_book_title(&raw_title);


        let author = obj.get("author").and_then(|v| v.as_str().map(|s| s.to_string()));
        let publisher = obj.get("publisher").and_then(|v| v.as_str().map(|s| s.to_string()));

        // Parse Year: string or integer
        let year = obj.get("year").and_then(|v| {
            if let Some(s) = v.as_str() {
                Some(s.to_string())
            } else if let Some(n) = v.as_i64() {
                Some(n.to_string())
            } else {
                None
            }
        });

        let language = obj.get("language").and_then(|v| v.as_str().map(|s| s.to_string()));
        let extension = obj.get("extension").and_then(|v| v.as_str().map(|s| s.to_string()));

        // Parse Filesize: u64 or string
        let filesize = obj.get("filesize").and_then(|v| {
            if let Some(n) = v.as_u64() {
                Some(n)
            } else if let Some(s) = v.as_str() {
                s.parse::<u64>().ok()
            } else {
                None
            }
        });

        let filesize_string = obj.get("filesize_string")
            .or_else(|| obj.get("filesizeReport"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));

        let cover = obj.get("cover").and_then(|v| v.as_str().map(|s| s.to_string()));
        let hash = obj.get("hash").and_then(|v| v.as_str().map(|s| s.to_string()));
        let description = obj.get("description").and_then(|v| v.as_str().map(|s| s.to_string()));

        let download_url = obj.get("dlUrl")
            .or_else(|| obj.get("url"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));

        Some(Book {
            id,
            title,
            author,
            publisher,
            year,
            language,
            extension,
            filesize,
            filesize_string,
            cover,
            hash,
            description,
            rating: None,
            quality: None,
            download_url,
        })
    }
}

/// Search request parameters.
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    pub query: String,
    pub page: u32,
    pub limit: u32,
    pub year_from: Option<u32>,
    pub year_to: Option<u32>,
    pub language: Option<String>,
    pub extension: Option<String>,
}

/// Search API Response payload from `/eapi/book/search`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponseData {
    pub books: Vec<Book>,
    #[serde(alias = "totalBooks")]
    pub total_books: Option<u64>,
}

/// Download link payload returned when requesting a book file download.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadResponseData {
    pub url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_login_response_deserialization() {
        let json_data = r#"{
            "user": {
                "id": 123456,
                "email": "test@example.com",
                "name": "Test User",
                "downloadsToday": 2,
                "downloadsLimit": 10,
                "remix_userid": "123456",
                "remix_userkey": "abcdef1234567890"
            }
        }"#;

        let res: LoginResponseData = serde_json::from_str(json_data).unwrap();
        let user = res.user.unwrap();
        assert_eq!(user.id, Some(123456));
        assert_eq!(user.downloads_today, Some(2));
        assert_eq!(user.downloads_limit, Some(10));
        assert_eq!(user.remix_userid.as_deref(), Some("123456"));
    }

    #[test]
    fn test_resilient_book_parser() {
        let json_val: serde_json::Value = serde_json::from_str(r#"{
            "id": "98765",
            "title": "Rust Programming in Action",
            "author": "Jane Doe",
            "year": 2024,
            "extension": "epub",
            "filesize": "1048576",
            "hash": "a1b2c3d4e5f6"
        }"#).unwrap();

        let book = Book::from_json_value(&json_val).unwrap();
        assert_eq!(book.id, 98765);
        assert_eq!(book.title, "Rust Programming in Action");
        assert_eq!(book.year.as_deref(), Some("2024"));
        assert_eq!(book.filesize, Some(1048576));
        assert_eq!(book.extension.as_deref(), Some("epub"));
        assert_eq!(book.hash.as_deref(), Some("a1b2c3d4e5f6"));
    }

    #[test]
    fn test_clean_book_title() {
        assert_eq!(
            clean_book_title("The Stars My Destination (A (z-library.sk, 1lib.sk, z-lib.sk)"),
            "The Stars My Destination"
        );
        assert_eq!(
            clean_book_title("There Is No Antimemetics Di_ (z-library.sk, 1lib.sk, z-lib.sk)"),
            "There Is No Antimemetics Di"
        );
        assert_eq!(
            clean_book_title("Rust Programming in Action (z-lib.is)"),
            "Rust Programming in Action"
        );
        assert_eq!(
            clean_book_title("Clean Code [z-lib.org]"),
            "Clean Code"
        );
        assert_eq!(
            clean_book_title("Normal Title"),
            "Normal Title"
        );
    }
}


