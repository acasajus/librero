use crate::client::ZLibraryClient;
use crate::config::Config;
use crate::db::Database;
use crate::email::{extract_epub_metadata, format_attachment_filename, EmailSender};
use crate::models::{clean_book_title, Book, SearchQuery};
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::collections::HashMap;
use teloxide::prelude::*;
use teloxide::types::{
    BotCommand, BotCommandScope, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, InputFile,
    ParseMode,
};
use tokio::fs;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tracing::{error, info, warn};

#[derive(Clone, Debug)]
pub enum PendingCustomEmail {
    NewDownload { book_id: u64, book_hash: String, chat_id: ChatId },
    ResendHistory { record_id: i64, chat_id: ChatId },
}

/// Shared Application State for Telegram Bot Handlers
#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub client: Arc<Mutex<ZLibraryClient>>,
    pub db: Database,
    pub email: EmailSender,
    pub pending_custom_emails: Arc<Mutex<HashMap<i64, PendingCustomEmail>>>,
    pub search_cache: Arc<Mutex<HashMap<i64, (String, Vec<Book>)>>>,
    pub history_cache: Arc<Mutex<HashMap<i64, (String, Vec<crate::db::DownloadRecord>)>>>,
}

/// Start the long-polling Telegram Bot daemon
pub async fn start_bot(state: AppState) -> Result<()> {
    let token = state.config.telegram.bot_token.clone();
    info!("Starting Telegram Bot daemon...");

    let bot = Bot::new(token);

    // Register Telegram Bot Command Menu in Telegram UI (Default & AllPrivateChats scope)
    let commands = vec![
        BotCommand::new("start", "Start bot and view Kindle setup guide"),
        BotCommand::new("search", "Search books on Z-Library"),
        BotCommand::new("kindle", "Kindle setup guide and send test file"),
        BotCommand::new("settings", "Configure 1-tap delivery preference"),
        BotCommand::new("doctor", "Check Tor, Z-Library quota and system health"),
        BotCommand::new("history", "View recent download history"),
        BotCommand::new("email", "View or set default Kindle email address"),
        BotCommand::new("help", "Display help and command menu"),
    ];

    if let Err(e) = bot.set_my_commands(commands.clone()).send().await {
        warn!("Failed to register Telegram bot command menu (default scope): {}", e);
    }
    if let Err(e) = bot.set_my_commands(commands.clone()).scope(BotCommandScope::AllPrivateChats).send().await {
        warn!("Failed to register Telegram bot command menu (private scope): {}", e);
    } else {
        info!("Successfully registered command menu with Telegram: /start, /search, /kindle, /settings, /doctor, /history, /email, /help");
    }

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

/// Handle incoming Telegram messages (`/start`, `/doctor`, `/history`, `/search`, plain queries)
async fn handle_message(bot: Bot, msg: Message, state: AppState) -> ResponseResult<()> {
    tokio::spawn(async move {
        if let Err(err) = process_message(bot, msg, state).await {
            error!("Error processing Telegram message: {:?}", err);
        }
    });
    Ok(())
}

async fn process_message(bot: Bot, msg: Message, state: AppState) -> ResponseResult<()> {
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

    // Check if user has a pending custom email prompt
    let pending_opt = {
        let mut map = state.pending_custom_emails.lock().await;
        map.remove(&user_id)
    };

    if let Some(pending) = pending_opt {
        if text.eq_ignore_ascii_case("/cancel") {
            bot.send_message(msg.chat.id, "❌ Custom email delivery cancelled.")
                .send()
                .await?;
            return Ok(());
        }

        if text.contains('@') && text.contains('.') && !text.starts_with('/') {
            let custom_email = text.to_string();
            bot.send_message(
                msg.chat.id,
                format!("✉️ Recipient email set to <code>{}</code>. Processing...", html_escape(&custom_email)),
            )
            .parse_mode(ParseMode::Html)
            .send()
            .await?;

            match pending {
                PendingCustomEmail::NewDownload { book_id, book_hash, chat_id } => {
                    execute_download_and_send(&bot, user_id, chat_id, &state, book_id, &book_hash, false, Some(custom_email)).await?;
                }
                PendingCustomEmail::ResendHistory { record_id, chat_id } => {
                    execute_resend_email(&bot, user_id, chat_id, &state, record_id, Some(custom_email)).await?;
                }
            }
            return Ok(());
        } else if !text.starts_with('/') {
            bot.send_message(
                msg.chat.id,
                "⚠️ Invalid email format. Please reply with a valid email (e.g. <code>mykindle@kindle.com</code>) or send <code>/cancel</code>.",
            )
            .parse_mode(ParseMode::Html)
            .send()
            .await?;

            state.pending_custom_emails.lock().await.insert(user_id, pending);
            return Ok(());
        }
    }

    let first_word = text.split_whitespace().next().unwrap_or("").to_lowercase();
    let base_cmd = first_word.split('@').next().unwrap_or("");

    match base_cmd {
        "/start" | "/help" => {
            let welcome_text = format!(
                "📚 <b>Welcome to Librero Bot!</b>\n\n\
                <b>Available Commands:</b>\n\
                • <code>/search &lt;query&gt;</code> - Search books on Z-Library\n\
                • <code>/kindle</code> - Kindle Setup Guide & Send Test File\n\
                • <code>/settings</code> - Configure 1-Tap Delivery Preference\n\
                • <code>/doctor</code> - Run system health diagnostics\n\
                • <code>/history</code> - View download history & re-send books\n\
                • <code>/email</code> - View or set your default Kindle email\n\
                • <code>/help</code> - Show this menu\n\n\
                <i>Send any book title or author directly to search!</i>"
            );
            bot.send_message(msg.chat.id, welcome_text)
                .parse_mode(ParseMode::Html)
                .send()
                .await?;
            Ok(())
        }
        "/kindle" => {
            let from_addr = &state.config.smtp.from_email;
            let text = format!(
                "📱 <b>Send-to-Kindle Setup Guide</b>\n\n\
                To receive books directly on your Kindle device, follow these 2 steps:\n\n\
                1️⃣ Open your Amazon account &gt; <b>Manage Your Content and Devices</b> &gt; <b>Preferences</b> &gt; <b>Personal Document Settings</b>.\n\
                2️⃣ Scroll to <b>Approved Personal Document E-mail List</b> and add:\n\
                <code>{}</code>\n\n\
                Click the button below to send a <b>Test File</b> to verify your Kindle setup!",
                html_escape(from_addr)
            );

            let keyboard = InlineKeyboardMarkup::new(vec![vec![
                InlineKeyboardButton::callback("🧪 Send Test File to Kindle", "send_kindle_test"),
            ]]);

            bot.send_message(msg.chat.id, text)
                .parse_mode(ParseMode::Html)
                .reply_markup(keyboard)
                .send()
                .await?;
            Ok(())
        }
        "/settings" => {
            let (pref, _) = state.db.get_user_setting(user_id).unwrap_or(("ask".into(), None));
            let fmt_pref = state.db.get_preferred_format(user_id);

            let pref_str = match pref.as_str() {
                "kindle" => "📧 Always Send to Kindle (1-Tap)",
                "telegram" => "💬 Always Upload to Telegram Chat",
                _ => "❓ Ask Every Time (Default)",
            };

            let text = format!(
                "⚙️ <b>Delivery & Format Preferences</b>\n\n\
                1️⃣ <b>1-Tap Delivery Mode:</b> <code>{}</code>\n\
                2️⃣ <b>Preferred Book Format:</b> <code>{}</code>\n\n\
                Select your preferences below:",
                pref_str,
                fmt_pref.to_uppercase()
            );

            let keyboard = InlineKeyboardMarkup::new(vec![
                vec![
                    InlineKeyboardButton::callback("📧 Always Kindle", "set_pref_kindle"),
                    InlineKeyboardButton::callback("💬 Always Telegram", "set_pref_telegram"),
                    InlineKeyboardButton::callback("❓ Ask Every Time", "set_pref_ask"),
                ],
                vec![
                    InlineKeyboardButton::callback("📘 Prefer EPUB", "set_fmt_epub"),
                    InlineKeyboardButton::callback("📄 Prefer PDF", "set_fmt_pdf"),
                    InlineKeyboardButton::callback("📖 Prefer MOBI", "set_fmt_mobi"),
                ],
            ]);

            bot.send_message(msg.chat.id, text)
                .parse_mode(ParseMode::Html)
                .reply_markup(keyboard)
                .send()
                .await?;
            Ok(())
        }
        "/doctor" | "doctor" => {
            handle_doctor_cmd(&bot, &msg, &state).await?;
            Ok(())
        }
        "/history" | "history" => {
            let filter_str = text.trim_start_matches("/history").trim();
            let filter_str = filter_str.split('@').next().unwrap_or(filter_str).trim();
            handle_history_cmd(&bot, &msg, &state, filter_str).await?;
            Ok(())
        }
        "/search" => {
            let query_str = text.trim_start_matches("/search").trim();
            let query_str = query_str.split('@').next().unwrap_or(query_str).trim();
            if query_str.is_empty() {
                bot.send_message(
                    msg.chat.id,
                    "💡 <b>Usage:</b> Send <code>/search &lt;title/author&gt;</code> (e.g. <code>/search rust programming</code>).",
                )
                .parse_mode(ParseMode::Html)
                .send()
                .await?;
                return Ok(());
            }
            handle_search_cmd(&bot, &msg, &state, query_str).await?;
            Ok(())
        }
        "/email" => {
            let arg = text.trim_start_matches("/email").trim();
            if arg.is_empty() {
                let current_email = state.config.find_user_email(user_id);
                let email_str = match current_email {
                    Some(ref e) if !e.trim().is_empty() => e.as_str(),
                    _ => "None configured",
                };
                bot.send_message(
                    msg.chat.id,
                    format!(
                        "📧 <b>Configured Email Address:</b> <code>{}</code>\n\n\
                        To send books to a custom address, click the <b>[ ✉️ Custom Email ]</b> button on any book card or send <code>/email &lt;address&gt;</code>.",
                        html_escape(email_str)
                    ),
                )
                .parse_mode(ParseMode::Html)
                .send()
                .await?;
            } else if arg.contains('@') && arg.contains('.') {
                bot.send_message(
                    msg.chat.id,
                    format!("✅ Default email address set to <code>{}</code>.", html_escape(arg)),
                )
                .parse_mode(ParseMode::Html)
                .send()
                .await?;
            } else {
                bot.send_message(
                    msg.chat.id,
                    "⚠️ Invalid email format. Usage: <code>/email mykindle@kindle.com</code>.",
                )
                .parse_mode(ParseMode::Html)
                .send()
                .await?;
            }
            Ok(())
        }
        _ => {
            if text.starts_with('/') {
                bot.send_message(
                    msg.chat.id,
                    format!("❌ Unknown command <code>{}</code>. Use <code>/help</code> or <code>/doctor</code>.", html_escape(base_cmd)),
                )
                .parse_mode(ParseMode::Html)
                .send()
                .await?;
                return Ok(());
            }
            let clean_query = extract_search_query_from_text(text);
            handle_search_cmd(&bot, &msg, &state, &clean_query).await?;
            Ok(())
        }
    }
}

fn extract_search_query_from_text(text: &str) -> String {
    let t = text.trim();
    if t.contains("amazon.com") || t.contains("goodreads.com") || t.contains("openlibrary.org") {
        if let Some(slug) = t.split('/').filter(|s| !s.is_empty()).last() {
            let clean_slug = slug.split('?').next().unwrap_or(slug);
            let words: Vec<&str> = clean_slug
                .split(&['-', '_', '+'][..])
                .filter(|w| w.len() > 2 && !w.chars().all(|c| c.is_numeric()))
                .collect();
            if !words.is_empty() {
                return words.join(" ");
            }
        }
    }
    t.to_string()
}

/// Perform health check diagnostics (`/doctor` command) in parallel
async fn handle_doctor_cmd(bot: &Bot, msg: &Message, state: &AppState) -> ResponseResult<()> {
    let user_obj = msg.from.as_ref().unwrap();
    info!("Running /doctor health check diagnostics in parallel for Telegram user {}", user_obj.id);

    let status_msg = bot.send_message(msg.chat.id, "🩺 <b>Running System Diagnostics...</b>")
        .parse_mode(ParseMode::Html)
        .send()
        .await?;

    let state_tor = state.clone();
    let state_email = state.clone();
    let state_db = state.clone();

    // Task 1: Check Tor & Z-Library profile concurrently over Tor
    let check_tor = tokio::spawn(async move {
        let client = state_tor.client.lock().await;
        client.get_profile().await
    });

    // Task 2: Check SMTP Server connection concurrently
    let check_smtp = tokio::task::spawn_blocking(move || {
        state_email.email.check_connection()
    });

    // Task 3: Check Turso/SQLite Database health concurrently
    let check_db = tokio::task::spawn_blocking(move || {
        state_db.db.get_total_downloads()
    });

    // Execute all 3 checks concurrently in parallel
    let (tor_res, smtp_res, db_res) = tokio::join!(check_tor, check_smtp, check_db);

    // Format Tor & Z-Library status
    let (tor_ok, profile_info) = match tor_res {
        Ok(Ok(prof)) => (
            "✅ Connected",
            format!(
                "Account: <b>{}</b>\nDownloads Today: <b>{} / {}</b>",
                html_escape(prof.name.as_deref().unwrap_or("Active")),
                prof.downloads_today.unwrap_or(0),
                prof.downloads_limit.unwrap_or(0)
            ),
        ),
        Ok(Err(e)) => ("❌ Error", format!("Connection failed: {}", html_escape(&e.to_string()))),
        Err(e) => ("❌ Task Error", format!("Tor check failed: {}", e)),
    };

    // Format SMTP Server status
    let smtp_status = match smtp_res {
        Ok(Ok(_)) => "✅ Connected".to_string(),
        Ok(Err(e)) => format!("❌ Disconnected ({})", html_escape(&e.to_string())),
        Err(e) => format!("❌ Task Error: {}", e),
    };

    // Format Database status
    let db_status = match db_res {
        Ok(Ok(cnt)) => format!("✅ Healthy ({} records logged)", cnt),
        Ok(Err(e)) => format!("❌ Database Error: {}", e),
        Err(e) => format!("❌ Task Error: {}", e),
    };

    let user_id = user_obj.id.0 as i64;
    let target_email = state.config.find_user_email(user_id).unwrap_or_else(|| "Not configured".into());

    let report = format!(
        "🏥 <b>Librero Doctor Health Report</b>\n\n\
        🌐 <b>Tor & Z-Library Connection:</b> {}\n{}\n\n\
        📧 <b>SMTP Server:</b> {}\n\
        📫 <b>Your Target Email:</b> <code>{}</code>\n\n\
        🗄️ <b>Turso/SQLite Database:</b> {}\n\n\
        🚀 <b>Status:</b> System operational and listening for orders.",
        tor_ok, profile_info, smtp_status, target_email, db_status
    );

    bot.edit_message_text(msg.chat.id, status_msg.id, report)
        .parse_mode(ParseMode::Html)
        .send()
        .await?;

    Ok(())
}

/// Render a single, paged history carousel message with 5 items per page & re-send buttons
fn render_history_results_page(
    records: &[crate::db::DownloadRecord],
    filter_query: &str,
    page: usize,
    per_page: usize,
) -> (String, InlineKeyboardMarkup) {
    let filtered_records: Vec<&crate::db::DownloadRecord> = if filter_query.trim().is_empty() {
        records.iter().collect()
    } else {
        let q = filter_query.to_lowercase();
        records
            .iter()
            .filter(|r| {
                r.book_title.to_lowercase().contains(&q)
                    || r.book_author.as_deref().unwrap_or("").to_lowercase().contains(&q)
                    || r.extension.as_deref().unwrap_or("").to_lowercase().contains(&q)
            })
            .collect()
    };

    let total_items = filtered_records.len();
    if total_items == 0 {
        let empty_text = if filter_query.is_empty() {
            "📜 <b>Your Download History</b> is empty.".to_string()
        } else {
            format!("🔍 No download history records matching <code>{}</code>.", html_escape(filter_query))
        };
        return (empty_text, InlineKeyboardMarkup::default());
    }

    let total_pages = (total_items + per_page - 1) / per_page;
    let current_page = page.min(total_pages).max(1);

    let start_idx = (current_page - 1) * per_page;
    let end_idx = (start_idx + per_page).min(total_items);
    let page_items = &filtered_records[start_idx..end_idx];

    let mut text = if filter_query.is_empty() {
        format!("📜 <b>Download History</b> (Page {} of {} • {} Total)\n\n", current_page, total_pages, total_items)
    } else {
        format!("🔍 <b>History Search for \"{}\"</b> (Page {} of {} • {} Total)\n\n", html_escape(filter_query), current_page, total_pages, total_items)
    };

    let mut action_rows = Vec::new();

    for (i, r) in page_items.iter().enumerate() {
        let num = start_idx + i + 1;
        let clean_title = clean_book_title(&r.book_title);
        let author = r.book_author.as_deref().unwrap_or("Unknown Author");
        let ext = r.extension.as_deref().unwrap_or("epub").to_uppercase();

        text.push_str(&format!(
            "<b>{}️⃣ {}</b>\n👤 <i>{}</i> • 📁 <code>{}</code>\n\n",
            num,
            html_escape(&clean_title),
            html_escape(author),
            ext
        ));

        let email_data = format!("resend_email_{}", r.id);
        let tg_data = format!("resend_tg_{}", r.id);

        action_rows.push(vec![
            InlineKeyboardButton::callback(format!("📧 Re-send #{:.3} Email", num), email_data),
            InlineKeyboardButton::callback(format!("💬 Re-send #{:.3} TG", num), tg_data),
        ]);
    }

    // Row for Pagination: [ ⬅️ Prev ] [ Page 1/3 ] [ Next ➡️ ]
    let mut nav_row = Vec::new();
    if current_page > 1 {
        nav_row.push(InlineKeyboardButton::callback(
            "⬅️ Prev",
            format!("hpage_{}", current_page - 1),
        ));
    }

    if total_pages > 1 {
        nav_row.push(InlineKeyboardButton::callback(
            format!("Page {}/{}", current_page, total_pages),
            "noop",
        ));
    }

    if current_page < total_pages {
        nav_row.push(InlineKeyboardButton::callback(
            "Next ➡️",
            format!("hpage_{}", current_page + 1),
        ));
    }

    if !nav_row.is_empty() {
        action_rows.push(nav_row);
    }

    (text, InlineKeyboardMarkup::new(action_rows))
}

/// View download history (`/history` command) with re-send & search filter
async fn handle_history_cmd(
    bot: &Bot,
    msg: &Message,
    state: &AppState,
    filter_query: &str,
) -> ResponseResult<()> {
    let user_obj = msg.from.as_ref().unwrap();
    let user_id = user_obj.id.0 as i64;

    info!("Retrieving download history for Telegram user {}", user_id);

    match state.db.get_user_history(user_id, 200) {
        Ok(records) => {
            state
                .history_cache
                .lock()
                .await
                .insert(user_id, (filter_query.to_string(), records.clone()));

            let (history_text, keyboard) = render_history_results_page(&records, filter_query, 1, 5);
            bot.send_message(msg.chat.id, history_text)
                .parse_mode(ParseMode::Html)
                .reply_markup(keyboard)
                .send()
                .await?;
        }
        Err(err) => {
            error!("Failed to fetch user history: {}", err);
            bot.send_message(
                msg.chat.id,
                format!("❌ Failed to retrieve history: {}", html_escape(&err.to_string())),
            )
            .parse_mode(ParseMode::Html)
            .send()
            .await?;
        }
    }

    Ok(())
}

/// Render a single, compact, paged carousel message with 5 books per page (up to 5 pages) & inline action buttons
fn render_search_results_page(
    query: &str,
    books: &[Book],
    page: usize,
    per_page: usize,
) -> (String, InlineKeyboardMarkup) {
    let total_books = books.len();
    let total_pages = ((total_books + per_page - 1) / per_page).min(5);
    let current_page = page.min(total_pages).max(1);

    let start_idx = (current_page - 1) * per_page;
    let end_idx = (start_idx + per_page).min(total_books);
    let page_books = &books[start_idx..end_idx];

    let mut text = format!(
        "📚 <b>Search Results for \"{}\"</b> (Page {} of {} • {} Total)\n\n",
        html_escape(query),
        current_page,
        total_pages,
        total_books
    );

    let mut action_rows = Vec::new();

    for (i, book) in page_books.iter().enumerate() {
        let num = start_idx + i + 1;
        let ext = book.extension.as_deref().unwrap_or("epub").to_uppercase();
        let size = book.filesize_string.as_deref().unwrap_or("N/A");
        let author = book.author.as_deref().unwrap_or("Unknown Author");
        let hash = book.hash.as_deref().unwrap_or("nohash");

        text.push_str(&format!(
            "<b>{}️⃣ {}</b>\n👤 <i>{}</i> • 📁 <code>{}</code> ({})\n\n",
            num,
            html_escape(&book.title),
            html_escape(author),
            ext,
            size
        ));

        let email_data = format!("dl_email_{}_{}", book.id, hash);
        let tg_data = format!("dl_tg_{}_{}", book.id, hash);
        let info_data = format!("info_{}_{}", book.id, hash);

        action_rows.push(vec![
            InlineKeyboardButton::callback(format!("📧 Kindle #{:.3}", num), email_data),
            InlineKeyboardButton::callback(format!("💬 TG #{:.3}", num), tg_data),
            InlineKeyboardButton::callback(format!("ℹ️ #{:.3}", num), info_data),
        ]);
    }

    // Row for Pagination: [ ⬅️ Prev ] [ Page 1/3 ] [ Next ➡️ ]
    let mut nav_row = Vec::new();
    if current_page > 1 {
        nav_row.push(InlineKeyboardButton::callback(
            "⬅️ Prev",
            format!("spage_{}", current_page - 1),
        ));
    }

    if total_pages > 1 {
        nav_row.push(InlineKeyboardButton::callback(
            format!("Page {}/{}", current_page, total_pages),
            "noop",
        ));
    }

    if current_page < total_pages {
        nav_row.push(InlineKeyboardButton::callback(
            "Next ➡️",
            format!("spage_{}", current_page + 1),
        ));
    }

    if !nav_row.is_empty() {
        action_rows.push(nav_row);
    }

    (text, InlineKeyboardMarkup::new(action_rows))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Execute book search with 30s timeout and retry option
async fn execute_search(bot: &Bot, chat_id: ChatId, state: &AppState, query: &str) -> ResponseResult<()> {
    let searching_msg = bot.send_message(chat_id, format!("🔍 Searching Z-Library over Tor for <code>{}</code>...", html_escape(query)))
        .parse_mode(ParseMode::Html)
        .send()
        .await?;

    let req = SearchQuery {
        query: query.to_string(),
        page: 1,
        limit: 20,
        ..Default::default()
    };

    let client = state.client.lock().await.clone();

    match timeout(Duration::from_secs(30), client.search(&req)).await {
        Ok(Ok(books)) => {
            if books.is_empty() {
                bot.edit_message_text(chat_id, searching_msg.id, format!("❌ No books found for <code>{}</code>.", html_escape(query)))
                    .parse_mode(ParseMode::Html)
                    .send()
                    .await?;
                return Ok(());
            }

            // Save search results in user's search cache for interactive pagination
            let target_user_id = searching_msg.chat.id.0;
            state.search_cache.lock().await.insert(target_user_id, (query.to_string(), books.clone()));

            let (page_text, keyboard) = render_search_results_page(query, &books, 1, 5);

            bot.edit_message_text(chat_id, searching_msg.id, page_text)
                .parse_mode(ParseMode::Html)
                .reply_markup(keyboard)
                .send()
                .await?;
        }
        Ok(Err(err)) => {
            error!("Search error: {}", err);
            let safe_q = query.chars().take(40).collect::<String>();
            let retry_data = format!("retry_search_{}", safe_q);
            let keyboard = InlineKeyboardMarkup::new(vec![vec![
                InlineKeyboardButton::callback("🔄 Retry Search", retry_data),
            ]]);

            bot.edit_message_text(
                chat_id,
                searching_msg.id,
                format!("❌ <b>Search Failed:</b> {}\n\nClick below to try again.", html_escape(&err.to_string())),
            )
            .parse_mode(ParseMode::Html)
            .reply_markup(keyboard)
            .send()
            .await?;
        }
        Err(_timeout) => {
            warn!("Search for query '{}' timed out after 30s", query);
            let safe_q = query.chars().take(40).collect::<String>();
            let retry_data = format!("retry_search_{}", safe_q);
            let keyboard = InlineKeyboardMarkup::new(vec![vec![
                InlineKeyboardButton::callback("🔄 Retry Search", retry_data),
            ]]);

            let timeout_text = format!(
                "⏱️ <b>Search Timed Out (30s)</b>\n\n\
                The Tor network or Z-Library onion mirror took too long to respond for <code>{}</code>.\n\n\
                Click below to retry or run <code>/doctor</code> to check connection health.",
                html_escape(query)
            );

            bot.edit_message_text(chat_id, searching_msg.id, timeout_text)
                .parse_mode(ParseMode::Html)
                .reply_markup(keyboard)
                .send()
                .await?;
        }
    }

    Ok(())
}

/// Execute book search command
async fn handle_search_cmd(bot: &Bot, msg: &Message, state: &AppState, query: &str) -> ResponseResult<()> {
    execute_search(bot, msg.chat.id, state, query).await
}

/// Handle inline keyboard button callbacks for book downloads (Email / Telegram) & re-sends
async fn handle_callback(bot: Bot, q: CallbackQuery, state: AppState) -> ResponseResult<()> {
    tokio::spawn(async move {
        if let Err(err) = process_callback(bot, q, state).await {
            error!("Error processing Telegram callback query: {:?}", err);
        }
    });
    Ok(())
}

async fn process_callback(bot: Bot, q: CallbackQuery, state: AppState) -> ResponseResult<()> {
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

    let chat_id = match q.message {
        Some(ref m) => m.chat().id,
        None => return Ok(()),
    };

    // Case 00: Search Pagination Callback (spage_<page_num>)
    if data.starts_with("spage_") {
        let target_page: usize = data.trim_start_matches("spage_").parse().unwrap_or(1);
        let cache = state.search_cache.lock().await;
        if let Some((ref query, ref books)) = cache.get(&user_id) {
            let (text, keyboard) = render_search_results_page(query, books, target_page, 5);
            if let Some(ref m) = q.message {
                bot.edit_message_text(chat_id, m.id(), text)
                    .parse_mode(ParseMode::Html)
                    .reply_markup(keyboard)
                    .send()
                    .await?;
            }
        }
        bot.answer_callback_query(&q.id).send().await?;
        return Ok(());
    }

    // Case 000: History Pagination Callback (hpage_<page_num>)
    if data.starts_with("hpage_") {
        let target_page: usize = data.trim_start_matches("hpage_").parse().unwrap_or(1);
        let cache = state.history_cache.lock().await;
        if let Some((ref filter_q, ref records)) = cache.get(&user_id) {
            let (text, keyboard) = render_history_results_page(records, filter_q, target_page, 5);
            if let Some(ref m) = q.message {
                bot.edit_message_text(chat_id, m.id(), text)
                    .parse_mode(ParseMode::Html)
                    .reply_markup(keyboard)
                    .send()
                    .await?;
            }
        }
        bot.answer_callback_query(&q.id).send().await?;
        return Ok(());
    }

    // Case 0000: Book Details & Synopsis Callback (info_<book_id>_<hash>)
    if data.starts_with("info_") {
        let parts: Vec<&str> = data.trim_start_matches("info_").split('_').collect();
        let book_id: u64 = parts.get(0).unwrap_or(&"0").parse().unwrap_or(0);
        let hash = parts.get(1).unwrap_or(&"nohash");

        bot.answer_callback_query(&q.id).text("⏳ Loading book details...").send().await?;

        let client = state.client.lock().await.clone();
        match client.get_download_info(book_id, hash).await {
            Ok(info) => {
                let clean_title = clean_book_title(&info.book.title);
                let author = info.book.author.as_deref().unwrap_or("Unknown Author");
                let publisher = info.book.publisher.as_deref().unwrap_or("Unknown Publisher");
                let year = info.book.year.as_deref().unwrap_or("N/A");
                let lang = info.book.language.as_deref().unwrap_or("N/A");
                let ext = info.book.extension.as_deref().unwrap_or("epub").to_uppercase();
                let size = info.book.filesize_string.as_deref().unwrap_or("N/A");
                let desc = info.book.description.as_deref().unwrap_or("No detailed synopsis available.");

                let details_text = format!(
                    "📖 <b>{}</b>\n\n\
                    👤 <b>Author:</b> {}\n\
                    🏢 <b>Publisher:</b> {}\n\
                    📅 <b>Year:</b> {} | 🌐 <b>Language:</b> {}\n\
                    📁 <b>Format:</b> {} ({})\n\n\
                    📝 <b>Synopsis / Summary:</b>\n<i>{}</i>",
                    html_escape(&clean_title),
                    html_escape(author),
                    html_escape(publisher),
                    html_escape(year),
                    html_escape(lang),
                    ext,
                    size,
                    html_escape(desc)
                );

                let email_data = format!("dl_email_{}_{}", book_id, hash);
                let tg_data = format!("dl_tg_{}_{}", book_id, hash);

                let keyboard = InlineKeyboardMarkup::new(vec![vec![
                    InlineKeyboardButton::callback("📧 Send to Kindle", email_data),
                    InlineKeyboardButton::callback("💬 Download to Telegram", tg_data),
                ]]);

                bot.send_message(chat_id, details_text)
                    .parse_mode(ParseMode::Html)
                    .reply_markup(keyboard)
                    .send()
                    .await?;
            }
            Err(e) => {
                bot.send_message(chat_id, format!("❌ Could not load details: {}", html_escape(&e.to_string()))).parse_mode(ParseMode::Html).send().await?;
            }
        }
        return Ok(());
    }

    if data == "noop" {
        bot.answer_callback_query(&q.id).send().await?;
        return Ok(());
    }

    // Case 0: Kindle Test File Send
    if data == "send_kindle_test" {
        bot.answer_callback_query(&q.id).text("Sending test file...").send().await?;
        let user_email = match state.config.find_user_email(user_id) {
            Some(e) if !e.is_empty() => e,
            _ => {
                bot.send_message(chat_id, "❌ No Kindle email configured. Use <code>/email &lt;address&gt;</code>.").parse_mode(ParseMode::Html).send().await?;
                return Ok(());
            }
        };

        let status_msg = bot.send_message(chat_id, format!("🧪 <b>Sending Kindle Test File to</b> <code>{}</code>...", html_escape(&user_email)))
            .parse_mode(ParseMode::Html).send().await?;

        let epub_bytes = crate::email::generate_kindle_test_epub();
        match state.email.send_book_attachment(
            &user_email,
            "Librero Kindle Setup Test",
            Some("Librero Bot"),
            "Librero Kindle Setup Test.epub",
            &epub_bytes,
            "epub",
        ) {
            Ok(_) => {
                bot.edit_message_text(
                    chat_id,
                    status_msg.id,
                    format!(
                        "🎉 <b>Kindle Test File Sent Successfully!</b>\n\n\
                        Check your Kindle device or Kindle app in a few minutes.\n\n\
                        <i>Note: If the book does not appear, ensure <code>{}</code> is added to your Amazon Approved Personal Document E-mail List!</i>",
                        html_escape(&state.config.smtp.from_email)
                    ),
                )
                .parse_mode(ParseMode::Html)
                .send()
                .await?;
            }
            Err(e) => {
                bot.edit_message_text(
                    chat_id,
                    status_msg.id,
                    format!("❌ <b>Kindle Test Send Failed:</b> {}", html_escape(&e.to_string())),
                )
                .parse_mode(ParseMode::Html)
                .send()
                .await?;
            }
        }
        return Ok(());
    }

    // Case 0b: 1-Tap Delivery Preference Setting
    if data.starts_with("set_pref_") {
        let pref = match data.as_str() {
            "set_pref_kindle" => "kindle",
            "set_pref_telegram" => "telegram",
            _ => "ask",
        };
        let _ = state.db.set_default_delivery(user_id, pref);
        let status_text = match pref {
            "kindle" => "✅ Preference updated: Always Send to Kindle (1-Tap Delivery enabled)",
            "telegram" => "✅ Preference updated: Always Upload to Telegram Chat",
            _ => "✅ Preference updated: Ask Every Time",
        };
        bot.answer_callback_query(&q.id).text(status_text).send().await?;
        bot.send_message(chat_id, status_text).send().await?;
        return Ok(());
    }

    // Case 0c: Preferred Format Setting
    if data.starts_with("set_fmt_") {
        let fmt = data.trim_start_matches("set_fmt_");
        let _ = state.db.set_preferred_format(user_id, fmt);
        let status_text = format!("✅ Preferred book format updated to: {}", fmt.to_uppercase());
        bot.answer_callback_query(&q.id).text(&status_text).send().await?;
        bot.send_message(chat_id, status_text).send().await?;
        return Ok(());
    }

    // Case 1: Re-send via Email from history (Default Email)
    if data.starts_with("resend_email_") {
        bot.answer_callback_query(&q.id).text("⏳ Processing email re-send...").send().await?;
        let db_id: i64 = data.trim_start_matches("resend_email_").parse().unwrap_or(0);
        return execute_resend_email(&bot, user_id, chat_id, &state, db_id, None).await;
    }

    // Case 2: Custom Email prompt for history re-send
    if data.starts_with("resend_custom_") {
        bot.answer_callback_query(&q.id).text("✉️ Send to Custom Email").send().await?;
        let db_id: i64 = data.trim_start_matches("resend_custom_").parse().unwrap_or(0);
        state.pending_custom_emails.lock().await.insert(user_id, PendingCustomEmail::ResendHistory {
            record_id: db_id,
            chat_id,
        });
        bot.send_message(
            chat_id,
            "✉️ <b>Re-send to Custom Email</b>\n\nPlease type the recipient email address (e.g. <code>friend@kindle.com</code> or <code>myemail@gmail.com</code>):\n\n<i>(Or send /cancel to abort)</i>",
        )
        .parse_mode(ParseMode::Html)
        .send()
        .await?;
        return Ok(());
    }

    // Case 3: Re-send via Telegram document from history
    if data.starts_with("resend_tg_") {
        bot.answer_callback_query(&q.id).text("⏳ Reading file for Telegram...").send().await?;

        let db_id: i64 = data.trim_start_matches("resend_tg_").parse().unwrap_or(0);
        let status_msg = bot.send_message(chat_id, format!("🔄 Preparing Telegram delivery for record #{}...", db_id))
            .parse_mode(ParseMode::Html)
            .send()
            .await?;

        let record = match state.db.get_record_by_id(db_id) {
            Ok(Some(r)) => r,
            _ => {
                bot.edit_message_text(chat_id, status_msg.id, "❌ History record not found in database.")
                    .parse_mode(ParseMode::Html)
                    .send()
                    .await?;
                return Ok(());
            }
        };

        let mut record = record;
        let file_path = PathBuf::from(&record.local_path);
        let ext = record.extension.as_deref().unwrap_or("epub");

        if file_path.exists() {
            if ext.eq_ignore_ascii_case("epub") {
                if let Ok(book_bytes) = fs::read(&file_path).await {
                    if let Some((epub_title, epub_author)) = extract_epub_metadata(&book_bytes) {
                        record.book_title = epub_title;
                        if epub_author.is_some() {
                            record.book_author = epub_author;
                        }
                    }
                }
            }

            let file_name = format_attachment_filename(&record.book_title, record.book_author.as_deref(), ext);
            let input_file = InputFile::file(&file_path).file_name(file_name);
            let caption = format!("📖 <b>{}</b>\n👤 Author: {}", html_escape(&record.book_title), html_escape(record.book_author.as_deref().unwrap_or("Unknown")));

            match bot.send_document(chat_id, input_file).caption(caption).parse_mode(ParseMode::Html).send().await {
                Ok(_) => {
                    bot.delete_message(chat_id, status_msg.id).send().await.ok();
                }
                Err(e) => {
                    bot.edit_message_text(chat_id, status_msg.id, format!("❌ Failed to send document to Telegram: {}", html_escape(&e.to_string())))
                        .parse_mode(ParseMode::Html)
                        .send()
                        .await?;
                }
            }
        } else {
            bot.edit_message_text(chat_id, status_msg.id, "❌ Local file missing from disk.").parse_mode(ParseMode::Html).send().await?;
        }
        return Ok(());
    }

    // Case 4: Delete record from history
    if data.starts_with("delete_hist_") {
        bot.answer_callback_query(&q.id).text("🗑️ Deleting record...").send().await?;

        let db_id: i64 = data.trim_start_matches("delete_hist_").parse().unwrap_or(0);

        let (book_title, local_path) = match state.db.get_record_by_id(db_id) {
            Ok(Some(r)) => (r.book_title.clone(), r.local_path.clone()),
            _ => ("Book".to_string(), String::new()),
        };

        match state.db.delete_record(db_id, user_id) {
            Ok(true) => {
                if !local_path.is_empty() {
                    let file_path = PathBuf::from(&local_path);
                    if file_path.exists() {
                        let _ = fs::remove_file(file_path).await;
                    }
                }

                if let Some(ref m) = q.message {
                    bot.edit_message_text(
                        m.chat().id,
                        m.id(),
                        format!("🗑️ <b>Deleted from History:</b> 📖 <i>{}</i>", html_escape(&book_title)),
                    )
                    .parse_mode(ParseMode::Html)
                    .send()
                    .await?;
                }
            }
            Ok(false) => {
                if let Some(ref m) = q.message {
                    bot.edit_message_text(m.chat().id, m.id(), "❌ Record not found or unauthorized.")
                        .parse_mode(ParseMode::Html)
                        .send()
                        .await?;
                }
            }
            Err(e) => {
                if let Some(ref m) = q.message {
                    bot.edit_message_text(m.chat().id, m.id(), format!("❌ Failed to delete record: {}", html_escape(&e.to_string())))
                        .parse_mode(ParseMode::Html)
                        .send()
                        .await?;
                }
            }
        }
        return Ok(());
    }

    // Case 5: Retry Search from timeout or error card
    if data.starts_with("retry_search_") {
        let query_str = data.trim_start_matches("retry_search_");
        bot.answer_callback_query(&q.id)
            .text(format!("🔄 Retrying search for '{}'...", query_str))
            .send()
            .await?;

        execute_search(&bot, chat_id, &state, query_str).await?;
        return Ok(());
    }

    // Case 6: Custom Email prompt for search download
    if data.starts_with("dl_custom_") {
        bot.answer_callback_query(&q.id).text("✉️ Send to Custom Email").send().await?;
        let payload = data.trim_start_matches("dl_custom_");
        let parts: Vec<&str> = payload.splitn(2, '_').collect();
        if parts.len() == 2 {
            if let Ok(b_id) = parts[0].parse::<u64>() {
                let b_hash = parts[1].to_string();
                state.pending_custom_emails.lock().await.insert(user_id, PendingCustomEmail::NewDownload {
                    book_id: b_id,
                    book_hash: b_hash,
                    chat_id,
                });
                bot.send_message(
                    chat_id,
                    "✉️ <b>Send to Custom Email</b>\n\nPlease type the recipient email address (e.g. <code>friend@kindle.com</code> or <code>myemail@gmail.com</code>):\n\n<i>(Or send /cancel to abort)</i>",
                )
                .parse_mode(ParseMode::Html)
                .send()
                .await?;
            }
        }
        return Ok(());
    }

    // Case 7: Download book and send (Default Email or Telegram)
    let is_tg_download = data.starts_with("dl_tg_");
    let is_email_download = data.starts_with("dl_email_") || data.starts_with("dl_");

    if !is_tg_download && !is_email_download {
        return Ok(());
    }

    bot.answer_callback_query(&q.id)
        .text("⏳ Starting download over Tor...")
        .send()
        .await?;

    let payload = if is_tg_download {
        data.trim_start_matches("dl_tg_")
    } else if data.starts_with("dl_email_") {
        data.trim_start_matches("dl_email_")
    } else {
        data.trim_start_matches("dl_")
    };

    let parts: Vec<&str> = payload.splitn(2, '_').collect();
    if parts.len() < 2 {
        return Ok(());
    }

    let book_id: u64 = match parts[0].parse() {
        Ok(id) => id,
        Err(_) => return Ok(()),
    };
    let book_hash = parts[1];

    execute_download_and_send(&bot, user_id, chat_id, &state, book_id, book_hash, is_tg_download, None).await
}

async fn execute_resend_email(
    bot: &Bot,
    user_id: i64,
    chat_id: ChatId,
    state: &AppState,
    db_id: i64,
    custom_email: Option<String>,
) -> ResponseResult<()> {
    let status_msg = bot.send_message(chat_id, format!("🔄 Preparing email re-send for record #{}...", db_id))
        .parse_mode(ParseMode::Html)
        .send()
        .await?;

    let record = match state.db.get_record_by_id(db_id) {
        Ok(Some(r)) => r,
        _ => {
            bot.edit_message_text(chat_id, status_msg.id, "❌ History record not found in database.")
                .parse_mode(ParseMode::Html)
                .send()
                .await?;
            return Ok(());
        }
    };

    let recipient_email = match custom_email {
        Some(ref e) if !e.trim().is_empty() => e.trim().to_string(),
        _ => match state.config.find_user_email(user_id) {
            Some(email) if !email.is_empty() => email,
            _ => record.user_email.clone(),
        },
    };

    if recipient_email.is_empty() {
        bot.edit_message_text(chat_id, status_msg.id, "❌ No email address configured. Use <code>/email &lt;address&gt;</code>.")
            .parse_mode(ParseMode::Html)
            .send()
            .await?;
        return Ok(());
    }

    let file_path = PathBuf::from(&record.local_path);
    let book_bytes = if file_path.exists() {
        match fs::read(&file_path).await {
            Ok(b) => b,
            Err(e) => {
                bot.edit_message_text(chat_id, status_msg.id, format!("❌ Failed to read file: {}", html_escape(&e.to_string())))
                    .parse_mode(ParseMode::Html)
                    .send()
                    .await?;
                return Ok(());
            }
        }
    } else {
        let client = state.client.lock().await.clone();
        let info = match client.get_download_info(record.book_id, "nohash").await {
            Ok(i) => i,
            Err(e) => {
                bot.edit_message_text(chat_id, status_msg.id, format!("❌ Re-download failed: {}", html_escape(&e.to_string())))
                    .parse_mode(ParseMode::Html)
                    .send()
                    .await?;
                return Ok(());
            }
        };
        match client.download_book_bytes(&info.url).await {
            Ok(b) => b,
            Err(e) => {
                bot.edit_message_text(chat_id, status_msg.id, format!("❌ Re-download failed: {}", html_escape(&e.to_string())))
                    .parse_mode(ParseMode::Html)
                    .send()
                    .await?;
                return Ok(());
            }
        }
    };

    let mut record = record;
    let ext = record.extension.as_deref().unwrap_or("epub");

    if ext.eq_ignore_ascii_case("epub") {
        if let Some((epub_title, epub_author)) = extract_epub_metadata(&book_bytes) {
            record.book_title = epub_title;
            if epub_author.is_some() {
                record.book_author = epub_author;
            }
        }
    }

    let attachment_name = format_attachment_filename(&record.book_title, record.book_author.as_deref(), ext);

    match state.email.send_book_attachment(&recipient_email, &record.book_title, record.book_author.as_deref(), &attachment_name, &book_bytes, ext) {
        Ok(_) => {
            let _ = state.db.update_sent_via_email(db_id, true);
            bot.edit_message_text(
                chat_id,
                status_msg.id,
                format!(
                    "✅ <b>Sent to Email!</b>\n\n📧 Delivered to <code>{}</code>\n📎 <b>Attachment:</b> <code>{}</code>",
                    html_escape(&recipient_email),
                    html_escape(&attachment_name)
                ),
            )
            .parse_mode(ParseMode::Html)
            .send()
            .await?;
        }
        Err(e) => {
            bot.edit_message_text(chat_id, status_msg.id, format!("❌ <b>SMTP Delivery Failed:</b> <code>{}</code>", html_escape(&e.to_string())))
                .parse_mode(ParseMode::Html)
                .send()
                .await?;
        }
    }

    Ok(())
}

async fn execute_download_and_send(
    bot: &Bot,
    user_id: i64,
    chat_id: ChatId,
    state: &AppState,
    book_id: u64,
    book_hash: &str,
    is_tg_download: bool,
    custom_email: Option<String>,
) -> ResponseResult<()> {
    let status_msg = bot.send_message(chat_id, format!("📥 Fetching download link for book ID {} over Tor...", book_id))
        .parse_mode(ParseMode::Html)
        .send()
        .await?;

    let (dl_info, book_bytes) = {
        let client = state.client.lock().await.clone();
        let info = match client.get_download_info(book_id, book_hash).await {
            Ok(i) => i,
            Err(e) => {
                bot.edit_message_text(chat_id, status_msg.id, format!("❌ Failed to get download URL: {}", html_escape(&e.to_string())))
                    .parse_mode(ParseMode::Html)
                    .send()
                    .await?;
                return Ok(());
            }
        };

        bot.edit_message_text(chat_id, status_msg.id, format!("⏬ Downloading <b>{}</b> over Tor...", html_escape(&info.book.title)))
            .parse_mode(ParseMode::Html)
            .send()
            .await
            .ok();

        let bytes = match client.download_book_bytes(&info.url).await {
            Ok(b) => b,
            Err(e) => {
                bot.edit_message_text(chat_id, status_msg.id, format!("❌ Download failed: {}", html_escape(&e.to_string())))
                    .parse_mode(ParseMode::Html)
                    .send()
                    .await?;
                return Ok(());
            }
        };

        (info, bytes)
    };

    let mut dl_info = dl_info;
    let ext = dl_info.book.extension.as_deref().unwrap_or("epub");

    if ext.eq_ignore_ascii_case("epub") {
        if let Some((epub_title, epub_author)) = extract_epub_metadata(&book_bytes) {
            info!("Extracted untruncated EPUB metadata: title='{}', author='{:?}'", epub_title, epub_author);
            dl_info.book.title = epub_title;
            if epub_author.is_some() {
                dl_info.book.author = epub_author;
            }
        }
    }

    let file_name = format_attachment_filename(&dl_info.book.title, dl_info.book.author.as_deref(), ext);

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

    // 3. Handle Delivery Target (Telegram document upload OR Email attachment)
    let recipient_email = match custom_email {
        Some(ref e) if !e.trim().is_empty() => e.trim().to_string(),
        _ => state.config.find_user_email(user_id).unwrap_or_default(),
    };
    let mut email_sent = false;

    if is_tg_download {
        bot.edit_message_text(chat_id, status_msg.id, "📄 Uploading file directly to Telegram chat...")
            .parse_mode(ParseMode::Html)
            .send()
            .await
            .ok();

        let input_file = InputFile::file(&local_file_path).file_name(file_name);
        let caption = format!("📖 <b>{}</b>\n👤 Author: {}", html_escape(&dl_info.book.title), html_escape(dl_info.book.author.as_deref().unwrap_or("Unknown")));

        if let Err(e) = bot.send_document(chat_id, input_file).caption(caption).parse_mode(ParseMode::Html).send().await {
            error!("Failed to send document to Telegram chat: {}", e);
        } else {
            bot.delete_message(chat_id, status_msg.id).send().await.ok();
        }
    } else {
        // Send via Email
        let attachment_name = format_attachment_filename(&dl_info.book.title, dl_info.book.author.as_deref(), ext);

        if !recipient_email.is_empty() {
            bot.edit_message_text(
                chat_id,
                status_msg.id,
                format!(
                    "📧 Sending book via SMTP to <code>{}</code>...\n📎 <b>Attachment:</b> <code>{}</code>",
                    html_escape(&recipient_email),
                    html_escape(&attachment_name)
                ),
            )
            .parse_mode(ParseMode::Html)
            .send()
            .await
            .ok();

            match state.email.send_book_attachment(&recipient_email, &dl_info.book.title, dl_info.book.author.as_deref(), &attachment_name, &book_bytes, ext) {
                Ok(_) => { email_sent = true; }
                Err(e) => { error!("Failed sending email to {}: {}", recipient_email, e); }
            }
        }

        let confirmation_text = format!(
            "✅ <b>Download Complete!</b>\n\n\
            📖 <b>Book:</b> {}\n\
            📁 <b>Saved Locally:</b> <code>{}</code>\n\
            📧 <b>Email:</b> {}\n\
            📎 <b>Attachment:</b> <code>{}</code>\n\
            📦 <b>Size:</b> {} bytes",
            html_escape(&dl_info.book.title),
            html_escape(&local_path_str),
            if email_sent { format!("Sent to <code>{}</code>", html_escape(&recipient_email)) } else { "Failed / Not configured".into() },
            html_escape(&attachment_name),
            book_bytes.len()
        );

        bot.edit_message_text(chat_id, status_msg.id, confirmation_text)
            .parse_mode(ParseMode::Html)
            .send()
            .await?;
    }

    // 4. Record real book metadata entry in Turso DB
    let mut book_record = dl_info.book;
    book_record.filesize = Some(book_bytes.len() as u64);
    let _ = state.db.record_download(user_id, &recipient_email, &book_record, &local_path_str, email_sent);

    Ok(())
}
