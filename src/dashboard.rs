use crate::client::ZLibraryClient;
use crate::config::Config;
use crate::db::Database;
use crate::email::{extract_epub_cover, EmailSender};
use crate::models::{clean_book_title, SearchQuery};
use anyhow::Result;
use axum::{
    extract::{Path as AxumPath, Query as AxumQuery, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;
use tracing::{info, warn};

/// Shared state for Admin Web Dashboard
#[derive(Clone)]
pub struct DashboardState {
    pub db: Database,
    pub config: Config,
    pub client: Arc<Mutex<ZLibraryClient>>,
    pub email: EmailSender,
    pub server_name: String,
}

#[derive(Deserialize)]
pub struct WebSearchParams {
    pub q: Option<String>,
}

#[derive(Deserialize)]
pub struct WebSendEmailParams {
    pub id: u64,
    pub hash: Option<String>,
    pub email: String,
}

#[derive(Serialize)]
pub struct DashboardRecordJson {
    pub id: i64,
    pub telegram_user_id: i64,
    pub telegram_username: Option<String>,
    pub user_email: String,
    pub book_id: u64,
    pub book_title: String,
    pub book_author: Option<String>,
    pub extension: Option<String>,
    pub filesize: u64,
    pub local_path: String,
    pub sent_via_email: bool,
    pub downloaded_at: String,
    pub download_url: String,
}

/// Start Admin Web Dashboard server on `host:port`.
/// Automatically compiles Tailwind CSS using `pnpm build` in `./web` on service startup.
pub async fn start_dashboard_server(
    db: Database,
    config: Config,
    client: Arc<Mutex<ZLibraryClient>>,
    email: EmailSender,
    host: &str,
    port: u16,
    server_name: &str,
) -> Result<()> {
    // 1. Compile Tailwind CSS using pnpm in ./web directory on service startup
    let web_dir = PathBuf::from("web");
    if web_dir.join("package.json").exists() {
        info!("🔨 Compiling Tailwind CSS pages using pnpm in {:?}...", web_dir);
        let status = tokio::process::Command::new("pnpm")
            .arg("build")
            .current_dir(&web_dir)
            .status()
            .await;

        match status {
            Ok(s) if s.success() => {
                info!("✨ Tailwind CSS successfully compiled with pnpm!");
            }
            Ok(s) => {
                warn!("⚠️ pnpm build returned exit status: {}", s);
            }
            Err(e) => {
                warn!("⚠️ Could not execute 'pnpm build' (pnpm may not be installed in PATH): {}. Using pre-compiled/CDN Tailwind styles.", e);
            }
        }
    }

    let state = DashboardState {
        db,
        config,
        client,
        email,
        server_name: server_name.to_string(),
    };

    let app = Router::new()
        .route("/", get(serve_dashboard_page))
        .route("/styles.css", get(serve_tailwind_css))
        .route("/read/:id", get(serve_epub_reader))
        .route("/cover/:id", get(serve_cover_image))
        .route("/api/search", get(api_web_search))
        .route("/api/send-email", get(api_web_send_email))
        .route("/json/downloads", get(serve_json_downloads))
        .route("/download/:id", get(serve_local_file_download))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!(
        "📊 Tailwind CSS Admin Dashboard running on http://{} (Port: {})",
        addr, port
    );

    axum::serve(listener, app).await?;
    Ok(())
}

/// Serve Tailwind CSS Compiled Stylesheet (`GET /styles.css`)
async fn serve_tailwind_css() -> Response {
    let css_path = PathBuf::from("web/dist/styles.css");
    if css_path.exists() {
        if let Ok(css) = fs::read_to_string(&css_path).await {
            return ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], css).into_response();
        }
    }

    (
        StatusCode::MOVED_PERMANENTLY,
        [(header::LOCATION, "https://cdn.jsdelivr.net/npm/tailwindcss@2.2.19/dist/tailwind.min.css")],
        "Redirecting to Tailwind CSS CDN",
    )
        .into_response()
}

/// Serve cover image for a book (`GET /cover/:id`)
async fn serve_cover_image(
    State(state): State<DashboardState>,
    AxumPath(id): AxumPath<i64>,
) -> Response {
    if let Ok(Some(record)) = state.db.get_record_by_id(id) {
        let path = PathBuf::from(&record.local_path);
        let cover_jpg = path.with_extension("cover.jpg");
        let cover_png = path.with_extension("cover.png");

        if cover_jpg.exists() {
            if let Ok(bytes) = fs::read(&cover_jpg).await {
                return ([(header::CONTENT_TYPE, "image/jpeg")], bytes).into_response();
            }
        }
        if cover_png.exists() {
            if let Ok(bytes) = fs::read(&cover_png).await {
                return ([(header::CONTENT_TYPE, "image/png")], bytes).into_response();
            }
        }
        if path.exists() {
            if let Ok(bytes) = fs::read(&path).await {
                if let Some((ext, img_bytes)) = extract_epub_cover(&bytes) {
                    let dest = if ext == "png" { cover_png } else { cover_jpg };
                    let _ = fs::write(&dest, &img_bytes).await;
                    let mime = if ext == "png" { "image/png" } else { "image/jpeg" };
                    return ([(header::CONTENT_TYPE, mime)], img_bytes).into_response();
                }
            }
        }
    }

    (StatusCode::NOT_FOUND, "No cover image available").into_response()
}

/// Live Z-Library Tor Search API (`GET /api/search?q=...`)
async fn api_web_search(
    State(state): State<DashboardState>,
    AxumQuery(params): AxumQuery<WebSearchParams>,
) -> Response {
    let query_str = params.q.as_deref().unwrap_or("").trim();
    if query_str.is_empty() {
        return (StatusCode::BAD_REQUEST, "Query parameter 'q' is required").into_response();
    }

    let req = SearchQuery {
        query: query_str.to_string(),
        page: 1,
        limit: 20,
        ..Default::default()
    };

    let client = state.client.lock().await.clone();
    match client.search(&req).await {
        Ok(books) => Json(books).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Z-Library Tor Search Error: {}", e),
        )
            .into_response(),
    }
}

/// Send Book to Custom Email API from Web UI (`GET /api/send-email?id=...&hash=...&email=...`)
async fn api_web_send_email(
    State(state): State<DashboardState>,
    AxumQuery(params): AxumQuery<WebSendEmailParams>,
) -> Response {
    let target_email = params.email.trim();
    if target_email.is_empty() || !target_email.contains('@') {
        return (StatusCode::BAD_REQUEST, "Invalid target email address").into_response();
    }

    // Check if book exists in local database records first
    if let Ok(Some(record)) = state.db.get_record_by_id(params.id as i64) {
        let file_path = PathBuf::from(&record.local_path);
        if file_path.exists() {
            if let Ok(bytes) = fs::read(&file_path).await {
                let ext = record.extension.as_deref().unwrap_or("epub");
                let clean_title = clean_book_title(&record.book_title);
                let author_str = record.book_author.as_deref().unwrap_or("Unknown");
                let attachment_name = format!("{} ({}).{}", clean_title, author_str, ext);

                match state.email.send_book_attachment(
                    target_email,
                    &clean_title,
                    record.book_author.as_deref(),
                    &attachment_name,
                    &bytes,
                    ext,
                ) {
                    Ok(_) => return (StatusCode::OK, "Book successfully sent to email!").into_response(),
                    Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("SMTP Email Send Failed: {}", e)).into_response(),
                }
            }
        }
    }

    // Otherwise download over Tor and send
    let hash = params.hash.as_deref().unwrap_or("nohash");
    let client = state.client.lock().await.clone();

    let info = match client.get_download_info(params.id, hash).await {
        Ok(i) => i,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("Download info error: {}", e)).into_response(),
    };

    let bytes = match client.download_book_bytes(&info.url).await {
        Ok(b) => b,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("Download bytes error: {}", e)).into_response(),
    };

    let ext = info.book.extension.as_deref().unwrap_or("epub");
    let clean_title = clean_book_title(&info.book.title);
    let author_str = info.book.author.as_deref().unwrap_or("Unknown");
    let attachment_name = format!("{} ({}).{}", clean_title, author_str, ext);

    match state.email.send_book_attachment(
        target_email,
        &clean_title,
        info.book.author.as_deref(),
        &attachment_name,
        &bytes,
        ext,
    ) {
        Ok(_) => (StatusCode::OK, "Book successfully downloaded & sent to email!").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("SMTP Email Send Failed: {}", e)).into_response(),
    }
}

/// In-Browser EPUB Reader Page (`GET /read/:id`)
async fn serve_epub_reader(
    State(state): State<DashboardState>,
    AxumPath(id): AxumPath<i64>,
) -> Html<String> {
    let record = state.db.get_record_by_id(id).ok().flatten();
    let title = record
        .as_ref()
        .map(|r| clean_book_title(&r.book_title))
        .unwrap_or_else(|| format!("Book #{}", id));
    let ext = record
        .as_ref()
        .and_then(|r| r.extension.as_deref())
        .unwrap_or("epub")
        .to_lowercase();

    let is_pdf = ext == "pdf";

    let viewer_html = if is_pdf {
        format!(r#"<iframe src="/download/{}" class="w-full h-full border-0 rounded-xl"></iframe>"#, id)
    } else {
        r#"<div id="viewer" class="w-full h-full text-slate-100 flex items-center justify-center">
            <div id="loadingText" class="text-cyan-400 font-semibold animate-pulse">⏳ Unpacking & Rendering EPUB...</div>
        </div>"#.to_string()
    };

    let script = if is_pdf {
        "".to_string()
    } else {
        format!(
            r##"
    <script>
        fetch("/download/{id}")
            .then(res => {{
                if (!res.ok) throw new Error("HTTP " + res.status + " " + res.statusText);
                return res.arrayBuffer();
            }})
            .then(buffer => {{
                const loadingText = document.getElementById('loadingText');
                if (loadingText) loadingText.remove();

                const book = ePub(buffer);
                const rendition = book.renderTo("viewer", {{
                    width: "100%",
                    height: "100%",
                    flow: "paginated"
                }});

                // Register Reading Themes (Dark, Sepia, Light)
                rendition.themes.register("dark", {{
                    "body": {{ "background": "#0f172a !important", "color": "#f8fafc !important", "padding": "20px !important" }},
                    "p, div, span, h1, h2, h3, h4, h5, h6, li, a": {{ "color": "#f8fafc !important", "background": "transparent !important" }}
                }});
                rendition.themes.register("sepia", {{
                    "body": {{ "background": "#fbf0d9 !important", "color": "#5f4b32 !important", "padding": "20px !important" }},
                    "p, div, span, h1, h2, h3, h4, h5, h6, li, a": {{ "color": "#5f4b32 !important", "background": "transparent !important" }}
                }});
                rendition.themes.register("light", {{
                    "body": {{ "background": "#ffffff !important", "color": "#0f172a !important", "padding": "20px !important" }},
                    "p, div, span, h1, h2, h3, h4, h5, h6, li, a": {{ "color": "#0f172a !important", "background": "transparent !important" }}
                }});

                rendition.themes.select("dark");
                rendition.display();

                const viewerWrapper = document.getElementById('viewerCard');

                document.getElementById('themeDark').onclick = () => {{
                    rendition.themes.select("dark");
                    viewerWrapper.className = "w-full max-w-6xl h-[88vh] bg-slate-900 border border-slate-800 rounded-2xl shadow-2xl p-4 transition-all";
                }};
                document.getElementById('themeSepia').onclick = () => {{
                    rendition.themes.select("sepia");
                    viewerWrapper.className = "w-full max-w-6xl h-[88vh] bg-[#fbf0d9] border border-[#e6d5b8] rounded-2xl shadow-2xl p-4 transition-all";
                }};
                document.getElementById('themeLight').onclick = () => {{
                    rendition.themes.select("light");
                    viewerWrapper.className = "w-full max-w-6xl h-[88vh] bg-white border border-slate-200 rounded-2xl shadow-2xl p-4 transition-all";
                }};

                let fontSize = 100;
                document.getElementById('fontInc').onclick = () => {{ fontSize += 10; rendition.themes.fontSize(fontSize + "%"); }};
                document.getElementById('fontDec').onclick = () => {{ if(fontSize > 60) fontSize -= 10; rendition.themes.fontSize(fontSize + "%"); }};

                document.getElementById('prevPage').onclick = () => rendition.prev();
                document.getElementById('nextPage').onclick = () => rendition.next();

                document.addEventListener("keydown", (e) => {{
                    if (e.key === "ArrowLeft") rendition.prev();
                    if (e.key === "ArrowRight") rendition.next();
                }});

                rendition.on("relocated", (location) => {{
                    if (location.atEnd) document.getElementById('readingProgress').innerText = "100%";
                    else if (location.start && location.start.percentage) document.getElementById('readingProgress').innerText = Math.round(location.start.percentage * 100) + "%";
                }});
            }})
            .catch(err => {{
                document.getElementById('viewer').innerHTML = '<div class="text-red-400 font-semibold text-center p-6">❌ Error loading EPUB: ' + err + '</div>';
            }});
    </script>
        "##,
            id = id
        )
    };

    let html = format!(
        r##"<!DOCTYPE html>
<html lang="en" class="h-full bg-slate-950">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>📖 Reader - {title}</title>
    <script src="https://cdn.tailwindcss.com"></script>
    <script src="https://cdnjs.cloudflare.com/ajax/libs/jszip/3.1.5/jszip.min.js"></script>
    <script src="https://cdn.jsdelivr.net/npm/epubjs@0.3.88/dist/epub.min.js"></script>
    <style>
        #viewer {{ width: 100% !important; height: 100% !important; }}
        #viewer iframe {{ width: 100% !important; height: 100% !important; border: 0 !important; }}
        .epub-container {{ width: 100% !important; height: 100% !important; }}
        .epub-view {{ height: 100% !important; width: 100% !important; }}
    </style>
</head>
<body class="h-full bg-slate-950 text-slate-100 flex flex-col antialiased">
    <div class="bg-slate-900/90 backdrop-blur border-b border-slate-800 p-4 flex justify-between items-center px-6">
        <div class="flex items-center space-x-4">
            <a href="/" class="text-xs font-semibold text-cyan-400 hover:text-cyan-300 transition-colors">← Back to Dashboard</a>
            <h1 class="text-sm font-bold text-slate-100 truncate max-w-md">{title}</h1>
        </div>
        <div class="flex items-center space-x-4">
            <div class="flex items-center space-x-1 bg-slate-950 p-1 rounded-lg border border-slate-800">
                <button id="themeDark" class="px-2 py-0.5 text-xs bg-slate-800 text-slate-200 rounded hover:bg-slate-700">🌙 Dark</button>
                <button id="themeSepia" class="px-2 py-0.5 text-xs bg-[#fbf0d9] text-[#5f4b32] font-semibold rounded hover:opacity-90">📜 Sepia</button>
                <button id="themeLight" class="px-2 py-0.5 text-xs bg-white text-slate-900 font-semibold rounded hover:bg-slate-100">☀️ Light</button>
            </div>
            <div class="flex items-center space-x-1">
                <button id="fontDec" class="px-2.5 py-1 text-xs bg-slate-800 rounded border border-slate-700 hover:bg-slate-700 font-bold">A-</button>
                <button id="fontInc" class="px-2.5 py-1 text-xs bg-slate-800 rounded border border-slate-700 hover:bg-slate-700 font-bold">A+</button>
            </div>
            <span id="readingProgress" class="text-xs font-mono text-cyan-400 font-bold">0%</span>
        </div>
    </div>

    <div class="flex-1 relative flex items-center justify-center p-4">
        <button id="prevPage" class="absolute left-6 z-10 p-3 bg-slate-900/90 border border-slate-700 rounded-full hover:bg-slate-800 shadow-2xl transition-all">⬅️</button>
        <div id="viewerCard" class="w-full max-w-6xl h-[88vh] bg-slate-900 border border-slate-800 rounded-2xl shadow-2xl p-4 transition-all">
            {viewer_html}
        </div>
        <button id="nextPage" class="absolute right-6 z-10 p-3 bg-slate-900/90 border border-slate-700 rounded-full hover:bg-slate-800 shadow-2xl transition-all">➡️</button>
    </div>

    {script}
</body>
</html>"##,
        title = html_escape(&title),
        viewer_html = viewer_html,
        script = script
    );

    Html(html)
}

/// Serve Tailwind CSS HTML5 Admin Dashboard Page (`GET /`)
async fn serve_dashboard_page(State(state): State<DashboardState>) -> Html<String> {
    let records = state.db.get_all_downloads(1000).unwrap_or_default();

    let total_books = records.len();
    let total_bytes: u64 = records.iter().map(|r| r.filesize.unwrap_or(0)).sum();
    let total_mb = format!("{:.2} MB", (total_bytes as f64) / (1024.0 * 1024.0));

    let mut unique_users = std::collections::HashSet::new();
    for r in &records {
        unique_users.insert(r.telegram_user_id);
    }
    let total_users = unique_users.len();

    let mut table_rows_html = String::new();
    for r in &records {
        let clean_title = clean_book_title(&r.book_title);
        let author = r.book_author.as_deref().unwrap_or("Unknown Author");
        let ext = r.extension.as_deref().unwrap_or("epub").to_uppercase();
        let ext_lower = r.extension.as_deref().unwrap_or("epub").to_lowercase();
        let size_kb = format!("{:.1} KB", (r.filesize.unwrap_or(0) as f64) / 1024.0);

        let username = state
            .config
            .telegram
            .allowed_users
            .iter()
            .find(|u| u.user_id == r.telegram_user_id)
            .and_then(|u| u.username.as_deref())
            .unwrap_or("Unknown");

        let user_display = if username != "Unknown" {
            format!("@{} (ID: {})", html_escape(username), r.telegram_user_id)
        } else {
            format!("User ID: {}", r.telegram_user_id)
        };

        let delivery_badge = if r.sent_via_email {
            format!(
                r#"<span class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-blue-500/10 text-blue-400 border border-blue-500/20">📧 Email: {}</span>"#,
                html_escape(&r.user_email)
            )
        } else {
            r#"<span class="inline-flex items-center px-2.5 py-0.5 rounded-full text-xs font-medium bg-purple-500/10 text-purple-400 border border-purple-500/20">💬 Telegram Direct</span>"#.to_string()
        };

        let reader_link = if ext_lower == "epub" || ext_lower == "pdf" {
            format!(
                r#"<a href="/read/{}" target="_blank" class="inline-flex items-center px-3 py-1.5 border border-cyan-500/30 text-xs font-semibold rounded-lg text-cyan-400 bg-cyan-500/10 hover:bg-cyan-500/20 transition-all">📖 Read Online</a>"#,
                r.id
            )
        } else {
            "".to_string()
        };

        let cover_img = format!(
            r#"<img src="/cover/{}" class="w-9 h-12 object-cover rounded shadow border border-slate-800/80 flex-shrink-0" onerror="this.style.display='none'">"#,
            r.id
        );

        table_rows_html.push_str(&format!(
            r#"<tr class="hover:bg-slate-800/40 transition-colors border-b border-slate-800/60" data-ext="{ext_lower}" data-search="{search_key}">
                <td class="px-6 py-4 whitespace-nowrap text-sm font-mono text-slate-400">#{id}</td>
                <td class="px-6 py-4">
                    <div class="flex items-center space-x-3">
                        {cover_img}
                        <span class="px-2.5 py-1 text-xs font-bold font-mono rounded-md bg-cyan-500/10 text-cyan-400 border border-cyan-500/20">{ext}</span>
                        <div>
                            <div class="text-sm font-semibold text-slate-100">{title}</div>
                            <div class="text-xs text-slate-400">by {author}</div>
                        </div>
                    </div>
                </td>
                <td class="px-6 py-4">
                    <div class="text-sm font-medium text-slate-200">👤 {user_disp}</div>
                    <div class="mt-1">{delivery}</div>
                </td>
                <td class="px-6 py-4 whitespace-nowrap text-sm text-slate-300 font-mono">{size}</td>
                <td class="px-6 py-4 whitespace-nowrap text-xs text-slate-400">📅 {date}</td>
                <td class="px-6 py-4 whitespace-nowrap text-right text-sm space-x-2">
                    {reader}
                    <button onclick="promptCustomEmail({id})" class="inline-flex items-center px-3 py-1.5 border border-purple-500/30 text-xs font-semibold rounded-lg text-purple-300 bg-purple-500/10 hover:bg-purple-500/20 transition-all">✉️ Custom Email</button>
                    <a href="/download/{id}" class="inline-flex items-center px-3 py-1.5 border border-transparent text-xs font-semibold rounded-lg text-white bg-gradient-to-r from-cyan-500 to-blue-600 hover:from-cyan-400 hover:to-blue-500 shadow-md transition-all">⏬ Download</a>
                </td>
            </tr>"#,
            id = r.id,
            cover_img = cover_img,
            ext_lower = ext_lower,
            ext = ext,
            title = html_escape(&clean_title),
            author = html_escape(author),
            user_disp = user_display,
            delivery = delivery_badge,
            size = size_kb,
            date = &r.downloaded_at,
            reader = reader_link,
            search_key = html_escape(&format!("{} {} {} {} {}", clean_title, author, user_display, r.user_email, ext_lower).to_lowercase())
        ));
    }

    if records.is_empty() {
        table_rows_html = r#"<tr>
            <td colspan="6" class="px-6 py-12 text-center text-sm text-slate-400">
                📚 No downloads logged in history database yet.
            </td>
        </tr>"#
            .to_string();
    }

    let from_email = state.config.smtp.from_email.clone();

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en" class="h-full bg-slate-950">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{server_name}</title>
    <script src="https://cdn.tailwindcss.com"></script>
    <link rel="stylesheet" href="/styles.css">
    <link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap">
    <style>
        body {{ font-family: 'Inter', sans-serif; }}
    </style>
</head>
<body id="dashBody" class="h-full bg-slate-950 text-slate-100 antialiased p-6 md:p-10 transition-colors">
    <div class="max-w-7xl mx-auto space-y-8">
        <!-- Header -->
        <div class="flex flex-col md:flex-row md:items-center md:justify-between gap-4 border-b border-slate-800 pb-6">
            <div>
                <h1 class="text-3xl font-extrabold tracking-tight bg-gradient-to-r from-cyan-400 via-sky-400 to-purple-500 bg-clip-text text-transparent">
                    📊 {server_name}
                </h1>
                <p class="text-sm text-slate-400 mt-1">
                    Compiled with Tailwind CSS & pnpm &bull; Live Tor Search & Online Reader Portal
                </p>
            </div>
            <div class="flex flex-wrap items-center gap-3">
                <button onclick="copyKindleEmail('{from_email}')" class="inline-flex items-center px-3 py-1.5 rounded-xl text-xs font-semibold bg-purple-500/10 text-purple-300 border border-purple-500/20 hover:bg-purple-500/20 transition-all">
                    📋 Copy Sender Email: <code class="ml-1.5 text-purple-200">{from_email}</code>
                </button>
                <span class="inline-flex items-center px-3 py-1 rounded-full text-xs font-semibold bg-emerald-500/10 text-emerald-400 border border-emerald-500/20">
                    <span class="w-2 h-2 rounded-full bg-emerald-400 animate-pulse mr-2"></span> Tor Online
                </span>
            </div>
        </div>

        <!-- Metrics Grid -->
        <div class="grid grid-cols-1 md:grid-cols-3 gap-6">
            <div class="bg-slate-900/60 backdrop-blur border border-slate-800/80 rounded-2xl p-6 shadow-xl hover:border-cyan-500/40 transition-all">
                <div class="text-xs font-semibold text-slate-400 uppercase tracking-wider">Total Books Downloaded</div>
                <div class="text-3xl font-extrabold text-white mt-2 flex items-center">
                    <span class="mr-2">📚</span> {total_books}
                </div>
            </div>
            <div class="bg-slate-900/60 backdrop-blur border border-slate-800/80 rounded-2xl p-6 shadow-xl hover:border-cyan-500/40 transition-all">
                <div class="text-xs font-semibold text-slate-400 uppercase tracking-wider">Total Storage Used</div>
                <div class="text-3xl font-extrabold text-white mt-2 flex items-center">
                    <span class="mr-2">📦</span> {total_mb}
                </div>
            </div>
            <div class="bg-slate-900/60 backdrop-blur border border-slate-800/80 rounded-2xl p-6 shadow-xl hover:border-cyan-500/40 transition-all">
                <div class="text-xs font-semibold text-slate-400 uppercase tracking-wider">Active Telegram Users</div>
                <div class="text-3xl font-extrabold text-white mt-2 flex items-center">
                    <span class="mr-2">👥</span> {total_users}
                </div>
            </div>
        </div>

        <!-- Web Live Search Section -->
        <div class="bg-slate-900/80 border border-slate-800 rounded-2xl p-6 shadow-xl space-y-4">
            <h2 class="text-lg font-bold text-slate-100 flex items-center">
                🔍 Live Tor Z-Library Search Portal
            </h2>
            <div class="flex gap-3">
                <input type="text" id="webSearchInput" placeholder="Type title, author, or ISBN to search Z-Library over Tor..." 
                    class="flex-1 bg-slate-950 border border-slate-800 rounded-xl px-5 py-3 text-sm text-slate-100 placeholder-slate-500 focus:outline-none focus:ring-2 focus:ring-cyan-500 shadow-inner">
                <button id="webSearchBtn" class="px-6 py-3 bg-gradient-to-r from-cyan-500 to-blue-600 hover:from-cyan-400 hover:to-blue-500 text-white font-semibold text-sm rounded-xl shadow-md transition-all">
                    🔍 Search Tor
                </button>
            </div>
            <div id="webSearchResults" class="hidden grid grid-cols-1 md:grid-cols-2 gap-4 pt-4 border-t border-slate-800"></div>
        </div>

        <!-- Filter Pills & Search History -->
        <div class="space-y-4">
            <div class="flex flex-wrap items-center gap-2" id="formatFilters">
                <span class="text-xs font-semibold text-slate-400 uppercase tracking-wider mr-2">Filter Library:</span>
                <button data-filter="all" class="filter-btn px-4 py-2 text-xs font-semibold rounded-lg bg-cyan-500 text-white shadow-md border border-cyan-400/30 transition-all">
                    All Formats
                </button>
                <button data-filter="epub" class="filter-btn px-4 py-2 text-xs font-semibold rounded-lg bg-slate-900 border border-slate-800 text-slate-300 hover:border-cyan-500/40 transition-all">
                    📘 EPUB
                </button>
                <button data-filter="pdf" class="filter-btn px-4 py-2 text-xs font-semibold rounded-lg bg-slate-900 border border-slate-800 text-slate-300 hover:border-cyan-500/40 transition-all">
                    📄 PDF
                </button>
                <button data-filter="mobi" class="filter-btn px-4 py-2 text-xs font-semibold rounded-lg bg-slate-900 border border-slate-800 text-slate-300 hover:border-cyan-500/40 transition-all">
                    📖 MOBI
                </button>
            </div>
            <input type="text" id="dashSearch" placeholder="🔍 Filter library history by title, author, user ID, format, or email..." 
                class="w-full bg-slate-900/80 border border-slate-800 rounded-xl px-5 py-3.5 text-sm text-slate-100 placeholder-slate-500 focus:outline-none focus:ring-2 focus:ring-cyan-500 shadow-inner">
        </div>

        <!-- Download Table -->
        <div class="bg-slate-900/60 backdrop-blur border border-slate-800/80 rounded-2xl shadow-xl overflow-hidden">
            <div class="overflow-x-auto">
                <table class="min-w-full divide-y divide-slate-800">
                    <thead class="bg-slate-900/90">
                        <tr>
                            <th class="px-6 py-4 text-left text-xs font-semibold text-slate-400 uppercase tracking-wider">#</th>
                            <th class="px-6 py-4 text-left text-xs font-semibold text-slate-400 uppercase tracking-wider">Book Information</th>
                            <th class="px-6 py-4 text-left text-xs font-semibold text-slate-400 uppercase tracking-wider">Downloaded By</th>
                            <th class="px-6 py-4 text-left text-xs font-semibold text-slate-400 uppercase tracking-wider">File Size</th>
                            <th class="px-6 py-4 text-left text-xs font-semibold text-slate-400 uppercase tracking-wider">Downloaded At</th>
                            <th class="px-6 py-4 text-right text-xs font-semibold text-slate-400 uppercase tracking-wider">Actions & Reader</th>
                        </tr>
                    </thead>
                    <tbody id="dashTbody" class="divide-y divide-slate-800/60">
                        {table_rows}
                    </tbody>
                </table>
            </div>
        </div>
    </div>

    <script>
        let activeFilter = 'all';
        let searchQuery = '';

        function applyFilters() {{
            const rows = document.querySelectorAll('#dashTbody tr');
            rows.forEach(row => {{
                const ext = row.getAttribute('data-ext') || '';
                const searchKey = row.getAttribute('data-search') || '';
                
                const matchesFilter = (activeFilter === 'all' || ext === activeFilter);
                const matchesSearch = (!searchQuery || searchKey.includes(searchQuery));

                if (matchesFilter && matchesSearch) {{
                    row.style.display = '';
                }} else {{
                    row.style.display = 'none';
                }}
            }});
        }}

        document.querySelectorAll('.filter-btn').forEach(btn => {{
            btn.addEventListener('click', function() {{
                document.querySelectorAll('.filter-btn').forEach(b => {{
                    b.className = 'filter-btn px-4 py-2 text-xs font-semibold rounded-lg bg-slate-900 border border-slate-800 text-slate-300 hover:border-cyan-500/40 hover:text-white transition-all';
                }});
                this.className = 'filter-btn px-4 py-2 text-xs font-semibold rounded-lg bg-cyan-500 text-white shadow-md border border-cyan-400/30 transition-all';

                activeFilter = this.getAttribute('data-filter');
                applyFilters();
            }});
        }});

        document.getElementById('dashSearch').addEventListener('input', function(e) {{
            searchQuery = e.target.value.toLowerCase().trim();
            applyFilters();
        }});

        // Custom Email Prompt Modal Function
        function promptCustomEmail(bookId, hash = '') {{
            const email = prompt("Enter custom recipient email address (e.g. friend@kindle.com):");
            if (email && email.includes('@')) {{
                fetch(`/api/send-email?id=${{bookId}}&hash=${{hash}}&email=${{encodeURIComponent(email)}}`)
                    .then(res => res.text())
                    .then(msg => alert("✉️ " + msg))
                    .catch(err => alert("❌ Email Send Error: " + err));
            }}
        }}

        // Web Live Tor Search Handler
        document.getElementById('webSearchBtn').onclick = performWebSearch;
        document.getElementById('webSearchInput').onkeydown = (e) => {{ if (e.key === 'Enter') performWebSearch(); }};

        function performWebSearch() {{
            const q = document.getElementById('webSearchInput').value.trim();
            if (!q) return;

            const resultsContainer = document.getElementById('webSearchResults');
            resultsContainer.classList.remove('hidden');
            resultsContainer.innerHTML = '<div class="col-span-2 text-center py-6 text-cyan-400 font-semibold animate-pulse">🔍 Searching Z-Library over Tor network...</div>';

            fetch(`/api/search?q=${{encodeURIComponent(q)}}`)
                .then(r => r.json())
                .then(books => {{
                    if (!books || books.length === 0) {{
                        resultsContainer.innerHTML = '<div class="col-span-2 text-center py-6 text-slate-400">❌ No books found on Z-Library.</div>';
                        return;
                    }}
                    let html = '';
                    books.slice(0, 8).forEach(b => {{
                        const ext = (b.extension || 'epub').toUpperCase();
                        const hash = b.hash || 'nohash';
                        html += `
                            <div class="bg-slate-950 border border-slate-800 rounded-xl p-4 flex flex-col justify-between space-y-3">
                                <div>
                                    <div class="flex justify-between items-start">
                                        <h3 class="font-bold text-sm text-slate-100 truncate max-w-[220px]">${{b.title}}</h3>
                                        <span class="px-2 py-0.5 text-xs font-mono bg-cyan-500/10 text-cyan-400 border border-cyan-500/20 rounded">${{ext}}</span>
                                    </div>
                                    <p class="text-xs text-slate-400 mt-1">👤 ${{b.author || 'Unknown Author'}} • ${{b.filesize_string || ''}}</p>
                                </div>
                                <div class="flex gap-2 text-xs">
                                    <button onclick="promptCustomEmail(${{b.id}}, '${{hash}}')" class="flex-1 py-2 bg-purple-600/20 border border-purple-500/30 text-purple-300 font-semibold rounded-lg hover:bg-purple-600/30">✉️ Custom Email</button>
                                </div>
                            </div>
                        `;
                    }});
                    resultsContainer.innerHTML = html;
                }})
                .catch(err => {{
                    resultsContainer.innerHTML = `<div class="col-span-2 text-center py-6 text-red-400">❌ Web search failed: ${{err}}</div>`;
                }});
        function copyKindleEmail(email) {{
            navigator.clipboard.writeText(email);
            alert("📋 Sender Email copied to clipboard:\n" + email + "\n\nAdd this to your Amazon Approved Personal Document E-mail List!");
        }}
    </script>
</body>
</html>"#,
        server_name = html_escape(&state.server_name),
        total_books = total_books,
        total_mb = total_mb,
        total_users = total_users,
        table_rows = table_rows_html
    );

    Html(html)
}

/// Serve JSON Download History (`GET /json/downloads`)
async fn serve_json_downloads(State(state): State<DashboardState>) -> Json<Vec<DashboardRecordJson>> {
    let records = state.db.get_all_downloads(1000).unwrap_or_default();

    let json_list: Vec<DashboardRecordJson> = records
        .into_iter()
        .map(|r| {
            let username = state
                .config
                .telegram
                .allowed_users
                .iter()
                .find(|u| u.user_id == r.telegram_user_id)
                .and_then(|u| u.username.clone());

            DashboardRecordJson {
                id: r.id,
                telegram_user_id: r.telegram_user_id,
                telegram_username: username,
                user_email: r.user_email,
                book_id: r.book_id,
                book_title: clean_book_title(&r.book_title),
                book_author: r.book_author,
                extension: r.extension,
                filesize: r.filesize.unwrap_or(0),
                local_path: r.local_path,
                sent_via_email: r.sent_via_email,
                downloaded_at: r.downloaded_at,
                download_url: format!("/download/{}", r.id),
            }
        })
        .collect();

    Json(json_list)
}

/// Serve Local Binary File Download (`GET /download/:id`)
async fn serve_local_file_download(
    State(state): State<DashboardState>,
    AxumPath(id): AxumPath<i64>,
) -> Response {
    let record = match state.db.get_record_by_id(id) {
        Ok(Some(r)) => r,
        _ => {
            return (StatusCode::NOT_FOUND, "Book record not found").into_response();
        }
    };

    let file_path = PathBuf::from(&record.local_path);
    if !file_path.exists() {
        return (
            StatusCode::NOT_FOUND,
            format!("File missing from server disk at {:?}", file_path),
        )
            .into_response();
    }

    let file_bytes = match fs::read(&file_path).await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed reading file: {}", e),
            )
                .into_response();
        }
    };

    let ext = record.extension.as_deref().unwrap_or("epub");
    let mime_type = match ext.to_lowercase().as_str() {
        "epub" => "application/epub+zip",
        "pdf" => "application/pdf",
        "mobi" => "application/x-mobipocket-ebook",
        "azw3" => "application/vnd.amazon.mobi8-ebook",
        _ => "application/octet-stream",
    };

    let clean_title = clean_book_title(&record.book_title);
    let author_str = record.book_author.as_deref().unwrap_or("Unknown");
    let filename = format!("{} ({}).{}", clean_title, author_str, ext);

    (
        [
            (header::CONTENT_TYPE, mime_type),
            (
                header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"{}\"", filename.replace('"', "_")),
            ),
        ],
        file_bytes,
    )
        .into_response()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dashboard_html_escape() {
        assert_eq!(html_escape("User & <Test>"), "User &amp; &lt;Test&gt;");
    }
}
