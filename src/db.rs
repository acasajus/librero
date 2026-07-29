use crate::models::Book;
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
                book_title: row.get(4)?,
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
                book_title: row.get(4)?,
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
}
