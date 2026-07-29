use crate::client::ZLibraryClient;
use crate::config::Config;
use crate::db::Database;
use crate::email::EmailSender;
use crate::models::SearchQuery;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, ParseMode};
use tokio::fs;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

/// Shared Application State for Telegram Bot Handlers
#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub client: Arc<Mutex<ZLibraryClient>>,
    pub db: Database,
    pub email: EmailSender,
}

/// Start the long-polling Telegram Bot daemon
pub async fn start_bot(state: AppState) -> Result<()> {
    let token = state.config.telegram.bot_token.clone();
    info!("Starting Telegram Bot daemon...");

    let bot = Bot::new(token);

    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(handle_message))
        .branch(Update::filter_callback_query().endpoint(handle_callback));

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

/// Handle incoming Telegram messages (`doctor`, `/doctor`, search queries)
async fn handle_message(bot: Bot, msg: Message, state: AppState) -> ResponseResult<()> {
    let user = match msg.from {
        Some(ref u) => u,
        None => return Ok(()),
    };

    let user_id = user.id.0 as i64;

    // Access control check
    if !state.config.is_user_allowed(user_id) {
        warn!("Unauthorized access attempt from Telegram user ID {} (@{:?})", user_id, user.username);
        bot.send_message(msg.chat.id, "❌ Access Denied: You are not authorized to use this bot.")
            .send()
            .await?;
        return Ok(());
    }

    let text = match msg.text() {
        Some(t) => t.trim(),
        None => return Ok(()),
    };

    if text.eq_ignore_ascii_case("doctor") || text.eq_ignore_ascii_case("/doctor") {
        handle_doctor_cmd(&bot, &msg, &state).await?;
        return Ok(());
    }

    // Process as a search query
    let query_str = if text.starts_with("/search") {
        text.trim_start_matches("/search").trim()
    } else {
        text
    };

    if query_str.is_empty() {
        bot.send_message(
            msg.chat.id,
            "💡 *Usage:* Send any book title/author to search, or type `doctor` to check system health.",
        )
        .parse_mode(ParseMode::MarkdownV2)
        .send()
        .await?;
        return Ok(());
    }

    handle_search_cmd(&bot, &msg, &state, query_str).await?;
    Ok(())
}

/// Perform health check diagnostics (`doctor` command)
async fn handle_doctor_cmd(bot: &Bot, msg: &Message, state: &AppState) -> ResponseResult<()> {
    let user_obj = msg.from.as_ref().unwrap();
    info!("Running doctor health check diagnostics for Telegram user {}", user_obj.id);
    let status_msg = bot.send_message(msg.chat.id, "🩺 *Running System Diagnostics...*")
        .parse_mode(ParseMode::MarkdownV2)
        .send()
        .await?;

    let client = state.client.lock().await;

    // 1. Z-Library & Tor Status
    let profile_res = client.get_profile().await;
    let (tor_ok, profile_info) = match profile_res {
        Ok(prof) => (
            "✅ Connected",
            format!(
                "Account: {}\nDownloads Today: {} / {}",
                prof.name.unwrap_or_else(|| "Active".into()),
                prof.downloads_today.unwrap_or(0),
                prof.downloads_limit.unwrap_or(0)
            ),
        ),
        Err(e) => ("❌ Error", format!("Connection failed: {}", e)),
    };

    // 2. SMTP Health
    let smtp_status = match state.email.check_connection() {
        Ok(_) => "✅ Connected",
        Err(_) => "❌ Disconnected / Invalid Credentials",
    };

    // 3. Database Health
    let db_status = match state.db.get_total_downloads() {
        Ok(cnt) => format!("✅ Healthy ({} records logged)", cnt),
        Err(e) => format!("❌ Database Error: {}", e),
    };

    let user_id = user_obj.id.0 as i64;
    let target_email = state.config.find_user_email(user_id).unwrap_or_else(|| "Not configured".into());

    let report = format!(
        "🏥 *Librero Doctor Health Report*\n\n\
        🌐 *Tor & Z-Library Connection:* {}\n{}\n\n\
        📧 *Gmail SMTP Server:* {}\n\
        📫 *Your Target Email:* `{}`\n\n\
        🗄️ *Turso/SQLite Database:* {}\n\n\
        🚀 *Status:* System operational and listening for orders.",
        tor_ok, profile_info, smtp_status, target_email, db_status
    );

    bot.edit_message_text(msg.chat.id, status_msg.id, report)
        .parse_mode(ParseMode::MarkdownV2)
        .send()
        .await?;

    Ok(())
}

/// Execute book search and render results with inline download buttons
async fn handle_search_cmd(bot: &Bot, msg: &Message, state: &AppState, query: &str) -> ResponseResult<()> {
    let searching_msg = bot.send_message(msg.chat.id, format!("🔍 Searching Z-Library over Tor for `{}`...", query))
        .send()
        .await?;

    let req = SearchQuery {
        query: query.to_string(),
        page: 1,
        limit: 5,
        ..Default::default()
    };

    let client = state.client.lock().await;
    match client.search(&req).await {
        Ok(books) => {
            if books.is_empty() {
                bot.edit_message_text(msg.chat.id, searching_msg.id, format!("❌ No books found for `{}`.", query))
                    .send()
                    .await?;
                return Ok(());
            }

            bot.delete_message(msg.chat.id, searching_msg.id).send().await.ok();

            bot.send_message(msg.chat.id, format!("📚 *Found {} books for '{}':*", books.len(), query))
                .parse_mode(ParseMode::MarkdownV2)
                .send()
                .await?;

            for book in books {
                let ext = book.extension.as_deref().unwrap_or("N/A").to_uppercase();
                let size = book.filesize_string.as_deref().unwrap_or("N/A");
                let author = book.author.as_deref().unwrap_or("Unknown Author");
                let year = book.year.as_deref().unwrap_or("N/A");
                let hash = book.hash.as_deref().unwrap_or("nohash");

                let text = format!(
                    "📖 *{}*\n👤 Author: {}\n📅 Year: {} | 📁 Format: {} ({})",
                    book.title, author, year, ext, size
                );

                // Callback data payload: dl_<book_id>_<hash>
                let callback_data = format!("dl_{}_{}", book.id, hash);
                let button_label = format!("📥 Download {}", ext);

                let keyboard = InlineKeyboardMarkup::new(vec![vec![
                    InlineKeyboardButton::callback(button_label, callback_data),
                ]]);

                bot.send_message(msg.chat.id, text)
                    .parse_mode(ParseMode::MarkdownV2)
                    .reply_markup(keyboard)
                    .send()
                    .await?;
            }
        }
        Err(err) => {
            error!("Search error: {}", err);
            bot.edit_message_text(msg.chat.id, searching_msg.id, format!("❌ Search failed: {}", err))
                .send()
                .await?;
        }
    }

    Ok(())
}

/// Handle inline keyboard button callbacks for book downloads
async fn handle_callback(bot: Bot, q: CallbackQuery, state: AppState) -> ResponseResult<()> {
    let user_id = q.from.id.0 as i64;

    // Access control check
    if !state.config.is_user_allowed(user_id) {
        bot.answer_callback_query(&q.id)
            .text("❌ Unauthorized user")
            .send()
            .await?;
        return Ok(());
    }

    let data = match q.data {
        Some(ref d) => d,
        None => return Ok(()),
    };

    if !data.starts_with("dl_") {
        return Ok(());
    }

    bot.answer_callback_query(&q.id)
        .text("⏳ Starting download over Tor...")
        .send()
        .await?;

    let parts: Vec<&str> = data.trim_start_matches("dl_").splitn(2, '_').collect();
    if parts.len() < 2 {
        return Ok(());
    }

    let book_id: u64 = match parts[0].parse() {
        Ok(id) => id,
        Err(_) => return Ok(()),
    };
    let book_hash = parts[1];

    let chat_id = match q.message {
        Some(ref m) => m.chat().id,
        None => return Ok(()),
    };

    let status_msg = bot.send_message(chat_id, format!("📥 Fetching download link for book ID {} over Tor...", book_id))
        .send()
        .await?;

    // 1. Resolve download URL & fetch bytes over Tor
    let (dl_url, book_bytes, _book_info) = {
        let client = state.client.lock().await;
        let url = match client.get_download_url(book_id, book_hash).await {
            Ok(u) => u,
            Err(e) => {
                bot.edit_message_text(chat_id, status_msg.id, format!("❌ Failed to get download URL: {}", e))
                    .send()
                    .await?;
                return Ok(());
            }
        };

        bot.edit_message_text(chat_id, status_msg.id, "⏬ Downloading book binary file over Tor...").send().await.ok();

        let bytes = match client.download_book_bytes(&url).await {
            Ok(b) => b,
            Err(e) => {
                bot.edit_message_text(chat_id, status_msg.id, format!("❌ Download failed: {}", e))
                    .send()
                    .await?;
                return Ok(());
            }
        };

        (url, bytes, book_id)
    };

    let ext = if dl_url.contains(".epub") { "epub" } else if dl_url.contains(".pdf") { "pdf" } else { "bin" };
    let file_name = format!("book_{}.{}", book_id, ext);

    // 2. Save locally into ./downloads/<user_id>/<file_name>
    let user_dir = PathBuf::from(&state.config.storage.download_dir).join(user_id.to_string());
    if let Err(e) = fs::create_dir_all(&user_dir).await {
        error!("Failed to create user download directory: {}", e);
    }

    let local_file_path = user_dir.join(&file_name);
    let save_res = fs::write(&local_file_path, &book_bytes).await;
    let local_path_str = local_file_path.to_string_lossy().to_string();

    if let Err(ref e) = save_res {
        error!("Failed saving book locally: {}", e);
    } else {
        info!("Saved book locally to '{}'", local_path_str);
    }

    // 3. Send email attachment via Gmail SMTP to configured user email
    let recipient_email = state.config.find_user_email(user_id).unwrap_or_default();
    let email_sent = if !recipient_email.is_empty() {
        bot.edit_message_text(chat_id, status_msg.id, format!("📧 Sending book via SMTP to `{}`...", recipient_email))
            .parse_mode(ParseMode::MarkdownV2)
            .send()
            .await
            .ok();

        match state.email.send_book_attachment(
            &recipient_email,
            &format!("Book {}", book_id),
            &file_name,
            &book_bytes,
            ext,
        ) {
            Ok(_) => true,
            Err(e) => {
                error!("Failed sending email to {}: {}", recipient_email, e);
                false
            }
        }
    } else {
        false
    };

    // 4. Record entry in Turso DB
    let dummy_book = crate::models::Book {
        id: book_id,
        title: format!("Book {}", book_id),
        author: None,
        publisher: None,
        year: None,
        language: None,
        extension: Some(ext.to_string()),
        filesize: Some(book_bytes.len() as u64),
        filesize_string: None,
        cover: None,
        hash: Some(book_hash.to_string()),
        description: None,
        rating: None,
        quality: None,
        download_url: Some(dl_url),
    };

    let _ = state.db.record_download(user_id, &recipient_email, &dummy_book, &local_path_str, email_sent);

    // 5. Send final confirmation report to Telegram user
    let email_status_str = if email_sent {
        format!("Sent to `{}`", recipient_email)
    } else {
        "Failed / Not configured".to_string()
    };

    let confirmation_text = format!(
        "✅ *Download Complete\\!*\n\n\
        📁 *Saved Locally:* `{}`\n\
        📧 *Email:* {}\n\
        📦 *Size:* {} bytes",
        local_path_str, email_status_str, book_bytes.len()
    );

    bot.edit_message_text(chat_id, status_msg.id, confirmation_text)
        .parse_mode(ParseMode::MarkdownV2)
        .send()
        .await?;

    Ok(())
}
