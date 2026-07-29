# Librero Architectural Specifications & Design Decisions

## 1. Executive Overview

`librero` is an automated **Telegram Bot Daemon Service** written in Rust. It authenticates to **Z-Library hidden services on the Tor network (.onion)** via reverse-engineered **eAPI** REST endpoints, listens for orders from authorized Telegram users, saves books to local storage and a **Turso/SQLite database**, and delivers books as attachments via **Gmail SMTP**.

Primary gateway: `http://loginzlib2vrak5zzpcocc3ouizykn6k5qecgj2tzlnab5wcbqhembyd.onion`  
Fallback mirror: `http://bookszlibb74ugqojhzhg2a63w5i2atv5bqarulgczawnbmsb6s6qead.onion`

---

## 2. System Architecture

```
librero/
├── Cargo.toml                  # Dependencies & crate metadata
├── config.example.toml         # Up-to-date TOML configuration example template
├── .gitignore                  # Excludes config.toml, databases, and downloads
├── README.md                   # Quick start & Telegram command guide
├── PROJECT_SPECS.md            # Technical specifications & design decisions
└── src/
    ├── lib.rs                  # Crate root & module re-exports
    ├── main.rs                 # Service daemon entry point
    ├── bot.rs                  # Telegram Bot listener, commands & inline button callbacks
    ├── client.rs               # ZLibraryClient eAPI async driver & mirror fallbacks
    ├── config.rs               # TOML Configuration schema & reader
    ├── db.rs                   # Turso / SQLite database manager
    ├── email.rs                # Gmail SMTP email attachment sender
    ├── models.rs               # Serde JSON data structures & schemas
    └── tor.rs                  # Tor network transport & SOCKS5/Arti embedded modes
```

> [!IMPORTANT]
> **Configuration File Policy**:
> 1. Always update `config.example.toml` whenever configuration schemas or defaults change.
> 2. **NEVER read or inspect `config.toml`** to protect secret credentials (passwords, bot tokens, app passwords).

---

## 3. Detailed Specifications

### 3.1 Access Control & Telegram User Filtering
- Every message and inline callback query is checked against `config.telegram.allowed_users`.
- Unauthorized Telegram user IDs are blocked.

### 3.2 `doctor` Diagnostic Command
- Verifies Tor hidden service connectivity to Z-Library.
- Queries Z-Library account profile and daily download limits (`downloadsToday` / `downloadsLimit`).
- Verifies SMTP server connectivity.
- Verifies Turso / SQLite database health and total recorded downloads.

### 3.3 Book Download Pipeline
When an inline download button (`dl_<book_id>_<hash>`) is clicked by an authorized user:
1. **Tor Download**: Fetches download URL over Tor via Z-Library eAPI and streams the binary file.
2. **Local Storage**: Saves the file in `./downloads/<TELEGRAM_USER_ID>/<filename>`.
3. **Database Logging**: Inserts a record into the `downloads` table in the Turso/SQLite database.
4. **Gmail SMTP Delivery**: Emails the book file attachment to the user's configured recipient email address.
5. **Telegram Feedback**: Edits the Telegram message with download confirmation, local path, and email status.
