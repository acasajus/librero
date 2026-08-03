use crate::config::Config;
use crate::db::Database;
use crate::models::clean_book_title;
use anyhow::Result;
use axum::{
    extract::{Path as AxumPath, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Serialize;
use std::path::PathBuf;
use tokio::fs;
use tower_http::cors::CorsLayer;
use tracing::{info, warn};

/// Shared state for Admin Web Dashboard
#[derive(Clone)]
pub struct DashboardState {
    pub db: Database,
    pub config: Config,
    pub server_name: String,
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
        server_name: server_name.to_string(),
    };

    let app = Router::new()
        .route("/", get(serve_dashboard_page))
        .route("/styles.css", get(serve_tailwind_css))
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

    // Fallback CDN redirect if pnpm was not installed on local host
    (
        StatusCode::MOVED_PERMANENTLY,
        [(header::LOCATION, "https://cdn.jsdelivr.net/npm/tailwindcss@2.2.19/dist/tailwind.min.css")],
        "Redirecting to Tailwind CSS CDN",
    )
        .into_response()
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

        table_rows_html.push_str(&format!(
            r#"<tr class="hover:bg-slate-800/40 transition-colors border-b border-slate-800/60" data-search="{search_key}">
                <td class="px-6 py-4 whitespace-nowrap text-sm font-mono text-slate-400">#{id}</td>
                <td class="px-6 py-4">
                    <div class="flex items-center space-x-3">
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
                <td class="px-6 py-4 whitespace-nowrap text-right text-sm">
                    <a href="/download/{id}" class="inline-flex items-center px-3.5 py-1.5 border border-transparent text-xs font-semibold rounded-lg text-white bg-gradient-to-r from-cyan-500 to-blue-600 hover:from-cyan-400 hover:to-blue-500 shadow-md shadow-cyan-500/10 transition-all transform hover:-translate-y-0.5">
                        ⏬ Download File
                    </a>
                </td>
            </tr>"#,
            id = r.id,
            ext = ext,
            title = html_escape(&clean_title),
            author = html_escape(author),
            user_disp = user_display,
            delivery = delivery_badge,
            size = size_kb,
            date = &r.downloaded_at,
            search_key = html_escape(&format!("{} {} {} {}", clean_title, author, user_display, r.user_email).to_lowercase())
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
<body class="h-full bg-slate-950 text-slate-100 antialiased p-6 md:p-10">
    <div class="max-w-7xl mx-auto space-y-8">
        <!-- Header -->
        <div class="flex flex-col md:flex-row md:items-center md:justify-between gap-4 border-b border-slate-800 pb-6">
            <div>
                <h1 class="text-3xl font-extrabold tracking-tight bg-gradient-to-r from-cyan-400 via-sky-400 to-purple-500 bg-clip-text text-transparent">
                    📊 {server_name}
                </h1>
                <p class="text-sm text-slate-400 mt-1">
                    Compiled with Tailwind CSS & pnpm &bull; Download History & Attribution Dashboard
                </p>
            </div>
            <div class="flex items-center space-x-3">
                <span class="inline-flex items-center px-3 py-1 rounded-full text-xs font-semibold bg-emerald-500/10 text-emerald-400 border border-emerald-500/20">
                    <span class="w-2 h-2 rounded-full bg-emerald-400 animate-pulse mr-2"></span> Active Service
                </span>
            </div>
        </div>

        <!-- Metrics Grid -->
        <div class="grid grid-cols-1 md:grid-cols-3 gap-6">
            <div class="bg-slate-900/60 backdrop-blur border border-slate-800/80 rounded-2xl p-6 shadow-xl relative overflow-hidden group hover:border-cyan-500/40 transition-all">
                <div class="text-xs font-semibold text-slate-400 uppercase tracking-wider">Total Books Downloaded</div>
                <div class="text-3xl font-extrabold text-white mt-2 flex items-center">
                    <span class="mr-2">📚</span> {total_books}
                </div>
            </div>
            <div class="bg-slate-900/60 backdrop-blur border border-slate-800/80 rounded-2xl p-6 shadow-xl relative overflow-hidden group hover:border-cyan-500/40 transition-all">
                <div class="text-xs font-semibold text-slate-400 uppercase tracking-wider">Total Storage Used</div>
                <div class="text-3xl font-extrabold text-white mt-2 flex items-center">
                    <span class="mr-2">📦</span> {total_mb}
                </div>
            </div>
            <div class="bg-slate-900/60 backdrop-blur border border-slate-800/80 rounded-2xl p-6 shadow-xl relative overflow-hidden group hover:border-cyan-500/40 transition-all">
                <div class="text-xs font-semibold text-slate-400 uppercase tracking-wider">Active Telegram Users</div>
                <div class="text-3xl font-extrabold text-white mt-2 flex items-center">
                    <span class="mr-2">👥</span> {total_users}
                </div>
            </div>
        </div>

        <!-- Search Bar -->
        <div class="relative">
            <input type="text" id="dashSearch" placeholder="🔍 Search downloads by book title, author, user ID, or recipient email..." 
                class="w-full bg-slate-900/80 border border-slate-800 rounded-xl px-5 py-3.5 text-sm text-slate-100 placeholder-slate-500 focus:outline-none focus:ring-2 focus:ring-cyan-500 focus:border-transparent shadow-inner transition-all">
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
                            <th class="px-6 py-4 text-right text-xs font-semibold text-slate-400 uppercase tracking-wider">Local Download</th>
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
        document.getElementById('dashSearch').addEventListener('input', function(e) {{
            const query = e.target.value.toLowerCase().trim();
            const rows = document.querySelectorAll('#dashTbody tr');
            rows.forEach(row => {{
                const searchKey = row.getAttribute('data-search');
                if (!searchKey || searchKey.includes(query)) {{
                    row.style.display = '';
                }} else {{
                    row.style.display = 'none';
                }}
            }});
        }});
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
