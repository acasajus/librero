use serde::{Deserialize, Serialize};

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
    fn test_book_deserialization() {
        let json_data = r#"{
            "id": 98765,
            "title": "Rust Programming in Action",
            "author": "Jane Doe",
            "publisher": "Tech Press",
            "year": "2024",
            "extension": "epub",
            "filesize": 1048576,
            "filesize_string": "1.0 MB",
            "hash": "a1b2c3d4e5f6"
        }"#;

        let book: Book = serde_json::from_str(json_data).unwrap();
        assert_eq!(book.id, 98765);
        assert_eq!(book.title, "Rust Programming in Action");
        assert_eq!(book.extension.as_deref(), Some("epub"));
        assert_eq!(book.hash.as_deref(), Some("a1b2c3d4e5f6"));
    }
}
