# librero 📚

An automated **Telegram Bot Daemon Service** written in Rust that connects to **Z-Library over Tor (.onion)**, provides search and download features, stores books locally & in a **Turso/SQLite database**, and delivers downloaded books to your email via **Gmail SMTP**.

---

## 🌟 Key Features

1. **Telegram Bot Daemon Service**: Runs as a background service listening for orders from authorized Telegram users only.
2. **Access Control**: Strict filtering by Telegram User ID (`telegram.allowed_users`). Requests from unauthorized users are rejected.
3. **`doctor` Diagnostics Command**: Checks health of Tor connection to Z-Library `.onion` addresses, account login status & quota limits, Gmail SMTP delivery, and Turso DB.
4. **Search & Interactive Downloads**: Search books in Telegram and get inline keyboard buttons (`📥 Download (EPUB/PDF)`).
5. **Local Storage**: Saves downloaded books in a structured local path (`./downloads/<TELEGRAM_USER_ID>/<filename>`).
6. **Turso / SQLite Database**: Logs all download history into a local or remote Turso/SQLite database (`librero.db`).
7. **Gmail SMTP Delivery**: Sends downloaded books directly as attachments to the user's configured email address (e.g. Kindle or personal email).
8. **Tor & Embedded Fallback**: Connects via local Tor SOCKS5 proxy or pure Rust embedded Tor (`arti-client`).

---

## 🚀 Quick Start & Setup

### 1. Configuration (`config.toml`)

Create or update `config.toml` (an example template is provided in `config.example.toml`):

```toml
[auth]
email = "your_zlibrary_email@example.com"
password = "your_zlibrary_password"

[telegram]
bot_token = "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ"

[[telegram.allowed_users]]
user_id = 123456789             # Your Telegram numerical user ID
username = "myusername"
email = "mykindle@kindle.com"   # Target recipient email address

[smtp]
host = "smtp.gmail.com"
port = 587
username = "your_gmail@gmail.com"
password = "your_gmail_app_password"  # Gmail App Password
from_email = "your_gmail@gmail.com"

[storage]
download_dir = "./downloads"
turso_db_path = "librero.db"

[tor]
mode = "auto"
proxy_url = "socks5h://127.0.0.1:9050"
onion_address = "http://loginzlib2vrak5zzpcocc3ouizykn6k5qecgj2tzlnab5wcbqhembyd.onion"
fallback_onion_addresses = [
    "http://bookszlibb74ugqojhzhg2a63w5i2atv5bqarulgczawnbmsb6s6qead.onion"
]
connect_timeout_seconds = 45
```

### 2. Run the Daemon Service

```bash
cargo run
```

---

## 🤖 Telegram Bot Commands

| Command | Action |
| :--- | :--- |
| **`doctor`** or **`/doctor`** | Checks system health (Tor connection, Z-Library quota, Gmail SMTP, Turso DB) |
| **`rust programming`** or **`/search <query>`** | Searches Z-Library and returns interactive messages with `📥 Download` buttons |
| **`[📥 Download]` Button Click** | Downloads book over Tor, saves to `./downloads/<user_id>/`, records entry in Turso DB, and emails the file attachment via SMTP |
