# Librero - Project Specifications & Memory

## Overview
**Librero** is a Rust service daemon that bridges Telegram with Z-Library hidden services on the Tor network. It allows authorized users to search for books, view full metadata, download files directly to their Telegram client or send them as attachments via SMTP (Brevo, Gmail App Password, or custom SMTP), and manage their download history.

---

## 🏗️ Architecture & Component Design

```
                       +-------------------------------+
                       |   Telegram App (User Chat)    |
                       +---------------+---------------+
                                       |
                                       v
                       +---------------+---------------+
                       |     Librero Rust Daemon       |
                       |          (teloxide)           |
                       +-------+-------+-------+-------+
                               |       |       |
             +-----------------+       |       +-----------------+
             v                         v                         v
+------------+------------+  +---------+---------+  +------------+------------+
|  Embedded Arti Tor      |  | Turso / SQLite DB |  | SMTP Email Sender          |
|  (arti-client)          |  |  (librero.db)     |  | (lettre - Port 587)       |
+------------+------------+  +-------------------+  +-------------------------+
             |
             v
+------------+------------+
| Z-Library Onion Mirrors |
| (eAPI over Tor stream)  |
+-------------------------+
```

---

## 🔑 Core Features & Specifications

### 1. Tor Connection & Failover Strategy (`src/tor.rs`, `src/client.rs`)
- **Connection Mode**: Pure Rust embedded **Arti Tor** (`arti-client`) connecting directly to public Tor nodes (`mode = "embedded"` by default). No local system `tor` service required.
- **Primary Onion Mirror**: `http://loginzlib2vrak5zzpcocc3ouizykn6k5qecgj2tzlnab5wcbqhembyd.onion`
- **Fallback Mirror**: `http://bookszlibb74ugqojhzhg2a63w5i2atv5bqarulgczawnbmsb6s6qead.onion`
- **Automatic Retry Policy**: Retries up to 3 times per mirror with a 1.5-second backoff delay on Tor/network errors (`timeout`, `connection reset`, `circuit error`, HTTP `502`/`503`/`504`). Automatically fails over to secondary `.onion` mirrors.

### 2. Startup Sequence & Parallel Diagnostics (`src/main.rs`, `src/bot.rs`)
- **Immediate Startup**: Telegram Bot starts listening immediately upon service launch. Z-Library auto-login runs concurrently in a background task.
- **Parallel `/doctor` Command**: Executes all 3 health checks (Tor & Z-Library profile, SMTP server connection, Turso DB check) **concurrently in parallel** using `tokio::join!`.

### 3. Telegram Bot Commands & Interactive UX (`src/bot.rs`)
- **Command Menu Registration**: Automatically registers commands (`/start`, `/search`, `/doctor`, `/history`, `/help`) with Telegram UI.
- **Access Control**: Restricts bot access strictly to Telegram `user_id`s configured in `config.toml`.
- **Formatting**: HTML mode (`ParseMode::Html`) with HTML escaping.
- **Top 10 Full Book Cards**: Always displays up to 10 search results as full cards with complete untruncated titles, authors, publishers, years, languages, formats, and file sizes.
- **Dual Delivery Buttons**:
  - `[ 📧 Send Email (EPUB) ]`: Downloads file, saves locally to `./downloads/<user_id>/`, records in DB, and sends via SMTP.
  - `[ 💬 Send to Telegram ]`: Downloads file and uploads document directly into the Telegram chat (`bot.send_document`).

### 4. Database & Download History (`src/db.rs`, `src/bot.rs`)
- **Turso / SQLite DB (`librero.db`)**: Stores full metadata for every download (`telegram_user_id`, `user_email`, `book_id`, `book_title`, `book_author`, `extension`, `filesize`, `local_path`, `sent_via_email`, `downloaded_at`).
- **`/history` Command**: Lists recent downloads with real titles and authors, plus interactive re-send buttons:
  - `[ 📧 Re-send Email ]`: Re-delivers from local disk cache via SMTP.
  - `[ 💬 Send to Telegram ]`: Uploads document directly from local disk cache into Telegram chat.

### 5. SMTP Email Delivery (`src/email.rs`)
- Supports **Brevo**, **Gmail (with App Password)**, Outlook, or custom SMTP servers on port `587` (STARTTLS).
- Detailed error logging and Telegram status updates on delivery failure.

### 6. Development & Task Runner (`justfile`, `Makefile`, `watch.sh`)
- `just watch` / `make watch` / `./watch.sh`: Automatically watches source file changes and restarts `cargo run --mode embedded`.

---

## 🔒 Configuration & Privacy Rules

- **Format**: `config.toml` (TOML format).
- **Security Rule**: `config.toml` is ignored in `.gitignore`.
- **Policy**: **ALWAYS** update `config.example.toml`. **NEVER** read or modify `config.toml`.
