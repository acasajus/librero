use crate::models::{clean_book_title, Book};
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::sync::{Arc, Mutex};
use tracing::info;

/// Record representing a downloaded book entry in Turso / SQLite DB
#[derive(Debug, Clone)]
pub struct DownloadRecord {
    pub id: i64,
    pub telegram_user_id: i64,
    pub user_email: String,
    pub book_id: u64,
    pub book_title: String,
    pub book_author: Option<String>,
    pub extension: Option<String>,
    pub filesize: Option<u64>,
    pub local_path: String,
    pub sent_via_email: bool,
    pub downloaded_at: String,
}

/// Database manager for Turso / SQLite
#[derive(Clone, Debug)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl Database {
    /// Initialize database connection and create table schema
    pub fn new(db_path: &str) -> Result<Self> {
        info!("Initializing Turso/SQLite database at '{}'", db_path);
        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open SQLite database at {}", db_path))?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS downloads (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                telegram_user_id INTEGER NOT NULL,
                user_email TEXT NOT NULL,
                book_id INTEGER NOT NULL,
                book_title TEXT NOT NULL,
                book_author TEXT,
                extension TEXT,
                filesize INTEGER,
                local_path TEXT NOT NULL,
                sent_via_email BOOLEAN NOT NULL DEFAULT 0,
                downloaded_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );",
            [],
        )
        .context("Failed to create 'downloads' database table schema")?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS user_settings (
                telegram_user_id INTEGER PRIMARY KEY,
                default_delivery TEXT NOT NULL DEFAULT 'ask',
                custom_email TEXT
            );",
            [],
        )
        .context("Failed to create 'user_settings' database table schema")?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Record a book download event in the database
    pub fn record_download(
        &self,
        telegram_user_id: i64,
        user_email: &str,
        book: &Book,
        local_path: &str,
        sent_via_email: bool,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO downloads (
                telegram_user_id, user_email, book_id, book_title, book_author, extension, filesize, local_path, sent_via_email
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                telegram_user_id,
                user_email,
                book.id,
                book.title,
                book.author,
                book.extension,
                book.filesize,
                local_path,
                sent_via_email,
            ],
        )
        .context("Failed to insert download record into database")?;

        let id = conn.last_insert_rowid();
        info!("Inserted download record #{} for book ID {} (user: {})", id, book.id, telegram_user_id);
        Ok(id)
    }

    /// Query a specific download record by database ID
    pub fn get_record_by_id(&self, id: i64) -> Result<Option<DownloadRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, telegram_user_id, user_email, book_id, book_title, book_author, extension, filesize, local_path, sent_via_email, downloaded_at
             FROM downloads
             WHERE id = ?1",
        )?;

        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(DownloadRecord {
                id: row.get(0)?,
                telegram_user_id: row.get(1)?,
                user_email: row.get(2)?,
                book_id: row.get(3)?,
                book_title: clean_book_title(&row.get::<_, String>(4)?),
                book_author: row.get(5)?,
                extension: row.get(6)?,
                filesize: row.get(7)?,
                local_path: row.get(8)?,
                sent_via_email: row.get(9)?,
                downloaded_at: row.get(10)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Update email delivery status for a record
    pub fn update_sent_via_email(&self, id: i64, sent: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloads SET sent_via_email = ?1 WHERE id = ?2",
            params![sent, id],
        )?;
        Ok(())
    }

    /// Query recent download history for a specific Telegram user ID
    pub fn get_user_history(&self, telegram_user_id: i64, limit: usize) -> Result<Vec<DownloadRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, telegram_user_id, user_email, book_id, book_title, book_author, extension, filesize, local_path, sent_via_email, downloaded_at
             FROM downloads
             WHERE telegram_user_id = ?1
             ORDER BY id DESC
             LIMIT ?2",
        )?;

        let history_iter = stmt.query_map(params![telegram_user_id, limit as i64], |row| {
            Ok(DownloadRecord {
                id: row.get(0)?,
                telegram_user_id: row.get(1)?,
                user_email: row.get(2)?,
                book_id: row.get(3)?,
                book_title: clean_book_title(&row.get::<_, String>(4)?),
                book_author: row.get(5)?,
                extension: row.get(6)?,
                filesize: row.get(7)?,
                local_path: row.get(8)?,
                sent_via_email: row.get(9)?,
                downloaded_at: row.get(10)?,
            })
        })?;

        let mut records = Vec::new();
        for record in history_iter {
            records.push(record?);
        }
        Ok(records)
    }

    /// Query all download records in library across all users (for Calibre Content Server)
    pub fn get_all_downloads(&self, limit: usize) -> Result<Vec<DownloadRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, telegram_user_id, user_email, book_id, book_title, book_author, extension, filesize, local_path, sent_via_email, downloaded_at
             FROM downloads
             ORDER BY id DESC
             LIMIT ?1",
        )?;

        let history_iter = stmt.query_map(params![limit as i64], |row| {
            Ok(DownloadRecord {
                id: row.get(0)?,
                telegram_user_id: row.get(1)?,
                user_email: row.get(2)?,
                book_id: row.get(3)?,
                book_title: clean_book_title(&row.get::<_, String>(4)?),
                book_author: row.get(5)?,
                extension: row.get(6)?,
                filesize: row.get(7)?,
                local_path: row.get(8)?,
                sent_via_email: row.get(9)?,
                downloaded_at: row.get(10)?,
            })
        })?;

        let mut records = Vec::new();
        for record in history_iter {
            records.push(record?);
        }
        Ok(records)
    }


    /// Query total download count for health check
    pub fn get_total_downloads(&self) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let count: u64 = conn.query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Delete a download record by ID and Telegram User ID (ensuring ownership)
    pub fn delete_record(&self, id: i64, telegram_user_id: i64) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let rows = conn.execute(
            "DELETE FROM downloads WHERE id = ?1 AND telegram_user_id = ?2",
            params![id, telegram_user_id],
        )?;
        Ok(rows > 0)
    }

    /// Get user settings (default_delivery, custom_email)
    pub fn get_user_setting(&self, telegram_user_id: i64) -> Result<(String, Option<String>)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT default_delivery, custom_email FROM user_settings WHERE telegram_user_id = ?1",
        )?;
        let mut rows = stmt.query(params![telegram_user_id])?;
        if let Some(row) = rows.next()? {
            let delivery: String = row.get(0)?;
            let email: Option<String> = row.get(1)?;
            Ok((delivery, email))
        } else {
            Ok(("ask".to_string(), None))
        }
    }

    /// Set user default delivery preference ('ask', 'kindle', 'telegram')
    pub fn set_default_delivery(&self, telegram_user_id: i64, delivery: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO user_settings (telegram_user_id, default_delivery) VALUES (?1, ?2)
             ON CONFLICT(telegram_user_id) DO UPDATE SET default_delivery = excluded.default_delivery",
            params![telegram_user_id, delivery],
        )?;
        Ok(())
    }

    /// Set user custom email preference
    pub fn set_user_custom_email(&self, telegram_user_id: i64, email: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO user_settings (telegram_user_id, custom_email) VALUES (?1, ?2)
             ON CONFLICT(telegram_user_id) DO UPDATE SET custom_email = excluded.custom_email",
            params![telegram_user_id, email],
        )?;
        Ok(())
    }

    /// Set user preferred format ('epub', 'pdf', 'mobi', 'any')
    pub fn set_preferred_format(&self, telegram_user_id: i64, format: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO user_settings (telegram_user_id, preferred_format) VALUES (?1, ?2)
             ON CONFLICT(telegram_user_id) DO UPDATE SET preferred_format = excluded.preferred_format",
            params![telegram_user_id, format],
        )?;
        Ok(())
    }

    /// Get user preferred format
    pub fn get_preferred_format(&self, telegram_user_id: i64) -> String {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT preferred_format FROM user_settings WHERE telegram_user_id = ?1").ok().unwrap();
        stmt.query_row(params![telegram_user_id], |row| row.get(0)).unwrap_or_else(|_| "epub".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_delete_record() {
        let db = Database::new(":memory:").unwrap();
        let book = Book {
            id: 101,
            title: "Test Book".into(),
            author: Some("Author".into()),
            publisher: None,
            year: None,
            language: None,
            extension: Some("epub".into()),
            filesize: Some(500),
            filesize_string: None,
            cover: None,
            hash: None,
            description: None,
            rating: None,
            quality: None,
            download_url: None,
        };


        db.record_download(12345, "user@test.com", &book, "/tmp/test.epub", true).unwrap();
        let history = db.get_user_history(12345, 10).unwrap();
        assert_eq!(history.len(), 1);
        let id = history[0].id;

        // Try deleting with wrong user ID
        assert_eq!(db.delete_record(id, 99999).unwrap(), false);
        assert_eq!(db.get_user_history(12345, 10).unwrap().len(), 1);

        // Delete with correct user ID
        assert_eq!(db.delete_record(id, 12345).unwrap(), true);
        assert_eq!(db.get_user_history(12345, 10).unwrap().len(), 0);
    }
}


