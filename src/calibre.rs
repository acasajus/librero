use crate::db::Database;
use crate::models::clean_book_title;
use anyhow::Result;
use axum::{
    extract::{Path as AxumPath, Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;
use tower_http::cors::CorsLayer;
use tracing::info;


/// Shared state for Calibre Content Server
#[derive(Clone)]
pub struct CalibreServerState {
    pub db: Database,
    pub server_name: String,
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
}

#[derive(Serialize)]
pub struct JsonBook {
    pub id: i64,
    pub title: String,
    pub author: Option<String>,
    pub extension: Option<String>,
    pub filesize: u64,
    pub downloaded_at: String,
    pub download_url: String,
}

/// Start Calibre Content Server background task on `host:port`
pub async fn start_calibre_server(
    db: Database,
    host: &str,
    port: u16,
    server_name: &str,
) -> Result<()> {
    let state = CalibreServerState {
        db,
        server_name: server_name.to_string(),
    };

    let app = Router::new()
        .route("/", get(serve_web_catalog))
        .route("/opds", get(serve_opds_catalog))
        .route("/opds/books", get(serve_opds_books))
        .route("/opds/search", get(serve_opds_search))
        .route("/opds/opensearch.xml", get(serve_opensearch_xml))
        .route("/json/books", get(serve_json_books))
        .route("/ajax/books", get(serve_json_books))
        .route("/download/:id", get(serve_book_download))
        .route("/get/file/:id", get(serve_book_download))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!(
        "📚 Calibre Content Server is running on http://{} (OPDS: http://{}/opds)",
        addr, addr
    );

    axum::serve(listener, app).await?;
    Ok(())
}

/// Serve HTML5 Responsive Web Catalog (`GET /`)
async fn serve_web_catalog(State(state): State<CalibreServerState>) -> Html<String> {
    let records = state.db.get_all_downloads(500).unwrap_or_default();

    let mut book_cards_html = String::new();
    for r in &records {
        let clean_title = clean_book_title(&r.book_title);
        let author = r.book_author.as_deref().unwrap_or("Unknown Author");
        let ext = r.extension.as_deref().unwrap_or("epub").to_uppercase();
        let size_mb = format!("{:.2} MB", (r.filesize.unwrap_or(0) as f64) / (1024.0 * 1024.0));

        book_cards_html.push_str(&format!(
            r#"<div class="book-card" data-title="{title_lower}" data-author="{author_lower}">
                <div class="book-badge">{ext}</div>
                <div class="book-details">
                    <h3 class="book-title">{title}</h3>
                    <p class="book-author">👤 {author}</p>
                    <p class="book-meta">📦 {size} &nbsp;|&nbsp; 📅 {date}</p>
                </div>
                <a class="download-btn" href="/download/{id}">⏬ Download</a>
            </div>"#,
            id = r.id,
            title = html_escape(&clean_title),
            title_lower = html_escape(&clean_title.to_lowercase()),
            author = html_escape(author),
            author_lower = html_escape(&author.to_lowercase()),
            ext = ext,
            size = size_mb,
            date = &r.downloaded_at
        ));
    }

    if records.is_empty() {
        book_cards_html = r#"<div class="empty-state">
            <p>📚 No books in library yet.</p>
            <p>Download books via Telegram bot or search to populate Calibre Content Server!</p>
        </div>"#.to_string();
    }

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{server_name}</title>
    <link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap">
    <style>
        :root {{
            --bg-color: #0f172a;
            --card-bg: #1e293b;
            --accent: #6366f1;
            --accent-hover: #4f46e5;
            --text-main: #f8fafc;
            --text-sub: #94a3b8;
            --border: #334155;
        }}
        * {{ box-sizing: border-box; margin: 0; padding: 0; }}
        body {{
            font-family: 'Inter', sans-serif;
            background-color: var(--bg-color);
            color: var(--text-main);
            padding: 2rem 1rem;
            min-height: 100vh;
        }}
        .container {{
            max-width: 1000px;
            margin: 0 auto;
        }}
        header {{
            text-align: center;
            margin-bottom: 2rem;
        }}
        h1 {{
            font-size: 2.2rem;
            font-weight: 700;
            background: linear-gradient(135deg, #a855f7, #6366f1);
            -webkit-background-clip: text;
            -webkit-text-fill-color: transparent;
            margin-bottom: 0.5rem;
        }}
        p.subtitle {{
            color: var(--text-sub);
            font-size: 0.95rem;
        }}
        .opds-banner {{
            background: rgba(99, 102, 241, 0.1);
            border: 1px solid var(--accent);
            padding: 0.75rem 1rem;
            border-radius: 8px;
            margin-bottom: 1.5rem;
            text-align: center;
            font-size: 0.9rem;
        }}
        .opds-banner a {{
            color: #818cf8;
            font-weight: 600;
            text-decoration: none;
        }}
        .opds-banner a:hover {{ text-decoration: underline; }}
        .search-box {{
            width: 100%;
            padding: 0.85rem 1.25rem;
            font-size: 1rem;
            border-radius: 10px;
            border: 1px solid var(--border);
            background: #1e293b;
            color: white;
            margin-bottom: 2rem;
            outline: none;
            transition: border-color 0.2s;
        }}
        .search-box:focus {{ border-color: var(--accent); }}
        .book-grid {{
            display: grid;
            grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
            gap: 1.25rem;
        }}
        .book-card {{
            background: var(--card-bg);
            border: 1px solid var(--border);
            border-radius: 12px;
            padding: 1.25rem;
            display: flex;
            flex-direction: column;
            justify-content: space-between;
            position: relative;
            transition: transform 0.2s, box-shadow 0.2s;
        }}
        .book-card:hover {{
            transform: translateY(-3px);
            box-shadow: 0 10px 25px -5px rgba(0, 0, 0, 0.3);
        }}
        .book-badge {{
            position: absolute;
            top: 1rem;
            right: 1rem;
            background: #3b82f6;
            color: white;
            font-size: 0.75rem;
            font-weight: 700;
            padding: 0.25rem 0.5rem;
            border-radius: 6px;
        }}
        .book-title {{
            font-size: 1.1rem;
            font-weight: 600;
            margin-bottom: 0.5rem;
            padding-right: 2.5rem;
            line-height: 1.3;
        }}
        .book-author {{
            color: var(--text-sub);
            font-size: 0.9rem;
            margin-bottom: 0.5rem;
        }}
        .book-meta {{
            color: #64748b;
            font-size: 0.8rem;
            margin-bottom: 1.25rem;
        }}
        .download-btn {{
            display: inline-block;
            text-align: center;
            background: linear-gradient(135deg, #6366f1, #4f46e5);
            color: white;
            font-weight: 600;
            padding: 0.65rem 1rem;
            border-radius: 8px;
            text-decoration: none;
            transition: background 0.2s;
        }}
        .download-btn:hover {{ background: linear-gradient(135deg, #4f46e5, #4338ca); }}
        .empty-state {{
            grid-column: 1 / -1;
            text-align: center;
            padding: 3rem 1rem;
            color: var(--text-sub);
        }}
    </style>
</head>
<body>
    <div class="container">
        <header>
            <h1>📚 {server_name}</h1>
            <p class="subtitle">Calibre Content Server & OPDS e-Reader Catalog</p>
        </header>

        <div class="opds-banner">
            📱 <b>e-Reader / Mobile App OPDS Feed:</b> <a href="/opds" target="_blank">http://{host_header}/opds</a>
        </div>

        <input type="text" id="searchInput" class="search-box" placeholder="🔍 Search library by title or author...">

        <div class="book-grid" id="bookGrid">
            {book_cards}
        </div>
    </div>

    <script>
        document.getElementById('searchInput').addEventListener('input', function(e) {{
            const term = e.target.value.toLowerCase().trim();
            const cards = document.querySelectorAll('.book-card');
            cards.forEach(card => {{
                const title = card.getAttribute('data-title');
                const author = card.getAttribute('data-author');
                if (title.includes(term) || author.includes(term)) {{
                    card.style.display = 'flex';
                }} else {{
                    card.style.display = 'none';
                }}
            }});
        }});
    </script>
</body>
</html>"#,
        server_name = html_escape(&state.server_name),
        host_header = "0.0.0.0",
        book_cards = book_cards_html
    );

    Html(html)
}

/// Serve OPDS Catalog Navigation Atom XML Feed (`GET /opds`)
async fn serve_opds_catalog(State(state): State<CalibreServerState>) -> Response {
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opds="http://opds-spec.org/2010/catalog">
  <id>urn:uuid:librero-calibre-opds-root</id>
  <title>{server_name}</title>
  <updated>2026-08-03T00:00:00Z</updated>
  <link rel="self" href="/opds" type="application/atom+xml;profile=opds-catalog;kind=navigation"/>
  <link rel="start" href="/opds" type="application/atom+xml;profile=opds-catalog;kind=navigation"/>
  <link rel="search" href="/opds/opensearch.xml" type="application/opensearchdescription+xml"/>

  <entry>
    <title>All Downloaded Books</title>
    <id>urn:librero:opds:all-books</id>
    <updated>2026-08-03T00:00:00Z</updated>
    <content type="text">Browse all books downloaded in your Librero library</content>
    <link rel="http://opds-spec.org/sort/new" href="/opds/books" type="application/atom+xml;profile=opds-catalog;kind=acquisition"/>
  </entry>
</feed>"#,
        server_name = xml_escape(&state.server_name)
    );

    (
        [(
            header::CONTENT_TYPE,
            "application/atom+xml;profile=opds-catalog;kind=navigation;charset=utf-8",
        )],
        xml,
    )
        .into_response()
}

/// Serve OPDS Acquisition Feed for Books (`GET /opds/books`)
async fn serve_opds_books(State(state): State<CalibreServerState>) -> Response {
    let records = state.db.get_all_downloads(500).unwrap_or_default();
    render_opds_acquisition_feed(&state.server_name, &records)
}

/// Serve OPDS Search Atom XML Feed (`GET /opds/search?q=...`)
async fn serve_opds_search(
    State(state): State<CalibreServerState>,
    Query(query): Query<SearchQuery>,
) -> Response {
    let all = state.db.get_all_downloads(500).unwrap_or_default();
    let search_term = query.q.as_deref().unwrap_or("").to_lowercase();

    let filtered: Vec<_> = if search_term.is_empty() {
        all
    } else {
        all.into_iter()
            .filter(|r| {
                r.book_title.to_lowercase().contains(&search_term)
                    || r.book_author
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&search_term)
            })
            .collect()
    };

    render_opds_acquisition_feed(&format!("Search: {}", search_term), &filtered)
}

/// Serve OpenSearch Description Document XML (`GET /opds/opensearch.xml`)
async fn serve_opensearch_xml() -> Response {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.1/">
  <ShortName>Librero Search</ShortName>
  <Description>Search Librero Calibre Book Library</Description>
  <InputEncoding>UTF-8</InputEncoding>
  <OutputEncoding>UTF-8</OutputEncoding>
  <Url type="application/atom+xml;profile=opds-catalog;kind=acquisition" template="/opds/search?q={searchTerms}"/>
</OpenSearchDescription>"#;

    (
        [(
            header::CONTENT_TYPE,
            "application/opensearchdescription+xml;charset=utf-8",
        )],
        xml,
    )
        .into_response()
}

/// Serve JSON API (`GET /json/books`)
async fn serve_json_books(State(state): State<CalibreServerState>) -> Json<Vec<JsonBook>> {
    let records = state.db.get_all_downloads(500).unwrap_or_default();

    let books: Vec<JsonBook> = records
        .into_iter()
        .map(|r| JsonBook {
            id: r.id,
            title: clean_book_title(&r.book_title),
            author: r.book_author,
            extension: r.extension,
            filesize: r.filesize.unwrap_or(0),
            downloaded_at: r.downloaded_at,
            download_url: format!("/download/{}", r.id),
        })
        .collect();

    Json(books)
}

/// Serve Binary Book File Download (`GET /download/:id`)
async fn serve_book_download(
    State(state): State<CalibreServerState>,
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
            format!("Book file missing from server disk at {:?}", file_path),
        )
            .into_response();
    }

    let file_bytes = match fs::read(&file_path).await {
        Ok(bytes) => bytes,
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
    let attachment_filename = format!("{} ({}).{}", clean_title, author_str, ext);

    (
        [
            (header::CONTENT_TYPE, mime_type),
            (
                header::CONTENT_DISPOSITION,
                &format!(
                    "attachment; filename=\"{}\"",
                    attachment_filename.replace('"', "_")
                ),
            ),
        ],
        file_bytes,
    )
        .into_response()
}

/// Helper function to render OPDS Atom XML Acquisition Feed
fn render_opds_acquisition_feed(
    title: &str,
    records: &[crate::db::DownloadRecord],
) -> Response {
    let mut entries_xml = String::new();

    for r in records {
        let clean_title = clean_book_title(&r.book_title);
        let author = r.book_author.as_deref().unwrap_or("Unknown Author");
        let ext = r.extension.as_deref().unwrap_or("epub");
        let mime_type = match ext.to_lowercase().as_str() {
            "epub" => "application/epub+zip",
            "pdf" => "application/pdf",
            "mobi" => "application/x-mobipocket-ebook",
            "azw3" => "application/vnd.amazon.mobi8-ebook",
            _ => "application/octet-stream",
        };

        entries_xml.push_str(&format!(
            r#"  <entry>
    <title>{title}</title>
    <id>urn:librero:book:{id}</id>
    <author><name>{author}</name></author>
    <dc:creator>{author}</dc:creator>
    <updated>{date}</updated>
    <summary>Format: {ext} | Size: {size} bytes</summary>
    <link rel="http://opds-spec.org/acquisition" href="/download/{id}" type="{mime}"/>
  </entry>
"#,
            id = r.id,
            title = xml_escape(&clean_title),
            author = xml_escape(author),
            date = xml_escape(&r.downloaded_at),
            ext = ext.to_uppercase(),
            size = r.filesize.unwrap_or(0),
            mime = mime_type
        ));
    }

    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opds="http://opds-spec.org/2010/catalog">
  <id>urn:uuid:librero-calibre-acquisition-feed</id>
  <title>{feed_title}</title>
  <updated>2026-08-03T00:00:00Z</updated>
  <link rel="self" href="/opds/books" type="application/atom+xml;profile=opds-catalog;kind=acquisition"/>
  <link rel="start" href="/opds" type="application/atom+xml;profile=opds-catalog;kind=navigation"/>
  <link rel="search" href="/opds/opensearch.xml" type="application/opensearchdescription+xml"/>

{entries}
</feed>"#,
        feed_title = xml_escape(title),
        entries = entries_xml
    );

    (
        [(
            header::CONTENT_TYPE,
            "application/atom+xml;profile=opds-catalog;kind=acquisition;charset=utf-8",
        )],
        xml,
    )
        .into_response()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calibre_xml_and_html_escaping() {
        assert_eq!(xml_escape("Dune & 'More' <test>"), "Dune &amp; &apos;More&apos; &lt;test&gt;");
        assert_eq!(html_escape("Title: \"Rust\" & Co"), "Title: &quot;Rust&quot; &amp; Co");
    }
}

