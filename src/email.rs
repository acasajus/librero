use crate::config::SmtpSettings;
use anyhow::{anyhow, Context, Result};
use lettre::message::{header::ContentType, Attachment, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use tracing::{error, info, warn};

use std::io::{Cursor, Read};
use zip::ZipArchive;

/// Inspect EPUB file bytes for DRM protection (Adobe ADEPT, Readium LCP, W3C encryption)
pub fn check_epub_drm(file_bytes: &[u8]) -> Result<bool> {
    let cursor = Cursor::new(file_bytes);
    let mut archive = match ZipArchive::new(cursor) {
        Ok(archive) => archive,
        Err(_) => return Ok(false),
    };

    // 1. Check for META-INF/rights.xml
    if archive.by_name("META-INF/rights.xml").is_ok() {
        return Ok(true);
    }

    // 2. Check for META-INF/encryption.xml containing DRM content encryption
    if let Ok(mut enc_file) = archive.by_name("META-INF/encryption.xml") {
        let mut content = String::new();
        if enc_file.read_to_string(&mut content).is_ok() {
            let content_lower = content.to_lowercase();
            if content_lower.contains("adobe.com/adept")
                || content_lower.contains("readium.org")
                || content_lower.contains("fairplay")
                || (content_lower.contains("http://www.w3.org/2001/04/xmlenc#")
                    && !content_lower.contains("http://www.idpf.org/2008/embedding")
                    && !content_lower.contains("http://ns.adobe.com/pdf/enc#font-start"))
            {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

/// Extract untruncated title and author metadata directly from an EPUB file's OPF XML
pub fn extract_epub_metadata(file_bytes: &[u8]) -> Option<(String, Option<String>)> {
    let cursor = Cursor::new(file_bytes);
    let mut archive = ZipArchive::new(cursor).ok()?;

    // 1. Find OPF file path from META-INF/container.xml or search for .opf entry
    let mut opf_path = String::new();
    if let Ok(mut container_file) = archive.by_name("META-INF/container.xml") {
        let mut container_xml = String::new();
        if container_file.read_to_string(&mut container_xml).is_ok() {
            if let Some(pos) = container_xml.find("full-path=\"") {
                let rest = &container_xml[pos + 11..];
                if let Some(end_pos) = rest.find('"') {
                    opf_path = rest[..end_pos].to_string();
                }
            }
        }
    }

    if opf_path.is_empty() {
        for i in 0..archive.len() {
            if let Ok(file) = archive.by_index(i) {
                if file.name().to_lowercase().ends_with(".opf") {
                    opf_path = file.name().to_string();
                    break;
                }
            }
        }
    }

    if opf_path.is_empty() {
        return None;
    }

    // 2. Read OPF file contents
    let mut opf_file = archive.by_name(&opf_path).ok()?;
    let mut opf_content = String::new();
    opf_file.read_to_string(&mut opf_content).ok()?;

    // 3. Extract <dc:title>
    let title = extract_xml_tag_content(&opf_content, "dc:title")
        .or_else(|| extract_xml_tag_content(&opf_content, "title"))?;

    // 4. Extract <dc:creator>
    let author = extract_xml_tag_content(&opf_content, "dc:creator")
        .or_else(|| extract_xml_tag_content(&opf_content, "author"));

    let clean_t = clean_xml_text(&title);
    let clean_a = author.map(|a| clean_xml_text(&a)).filter(|a| !a.is_empty());

    if clean_t.is_empty() {
        None
    } else {
        Some((clean_t, clean_a))
    }
}

fn extract_xml_tag_content(xml: &str, tag: &str) -> Option<String> {
    let open_tag_start = format!("<{}", tag);
    let close_tag = format!("</{}>", tag);

    let lower_xml = xml.to_lowercase();
    let open_pos = lower_xml.find(&open_tag_start)?;
    let content_start_rel = xml[open_pos..].find('>')?;
    let content_start = open_pos + content_start_rel + 1;

    let close_pos = lower_xml[content_start..].find(&close_tag)?;
    let content = &xml[content_start..content_start + close_pos];

    let mut result = String::new();
    let mut in_tag = false;
    for c in content.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            result.push(c);
        }
    }

    let trimmed = result.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn clean_xml_text(s: &str) -> String {
    let unescaped = s
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'");
    clean_book_title(&unescaped)
}

/// Sanitize string for filesystem and email attachment filename safety

pub fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0'..='\x1F' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

use crate::models::clean_book_title;

/// Format attachment filename as 'book name (Author name).extension'
pub fn format_attachment_filename(title: &str, author: Option<&str>, ext: &str) -> String {
    let cleaned_title = clean_book_title(title);
    let clean_title = sanitize_filename(&cleaned_title);
    let clean_ext = ext.trim_start_matches('.');


    let name = match author.map(|a| a.trim()).filter(|a| !a.is_empty() && *a != "Unknown") {
        Some(a) => {
            let clean_author = sanitize_filename(a);
            let clean_title_lower = clean_title.to_lowercase();
            let clean_author_lower = clean_author.to_lowercase();

            if clean_title_lower.contains(&clean_author_lower) {
                clean_title
            } else {
                format!("{} ({})", clean_title, clean_author)
            }
        }
        None => clean_title,
    };


    if name.is_empty() {
        format!("book.{}", clean_ext)
    } else {
        format!("{}.{}", name, clean_ext)
    }
}

/// Email Sender Service using SMTP (Gmail, Brevo, custom SMTP)

#[derive(Clone, Debug)]
pub struct EmailSender {
    config: SmtpSettings,
}

impl EmailSender {
    pub fn new(config: SmtpSettings) -> Self {
        Self { config }
    }

    /// Build an SmtpTransport client using SMTP settings (STARTTLS on port 587)
    fn build_transport(&self) -> Result<SmtpTransport> {
        let creds = Credentials::new(self.config.username.clone(), self.config.password.clone());

        let transport = SmtpTransport::starttls_relay(&self.config.host)
            .context(format!("Failed to connect to SMTP host '{}'", self.config.host))?
            .port(self.config.port)
            .credentials(creds)
            .build();

        Ok(transport)
    }

    /// Check connection health to SMTP server
    pub fn check_connection(&self) -> Result<()> {
        if self.config.username.is_empty() || self.config.password.is_empty() {
            return Err(anyhow!("SMTP credentials not configured in config.toml"));
        }

        info!("Verifying SMTP connection to '{}:{}'...", self.config.host, self.config.port);
        let transport = self.build_transport()?;

        match transport.test_connection() {
            Ok(true) => {
                info!("SMTP server connection verified successfully");
                Ok(())
            }
            Ok(false) => {
                warn!("SMTP test connection returned false");
                Err(anyhow!("SMTP server test connection failed"))
            }
            Err(e) => {
                error!("SMTP connection/authentication test error: {}", e);
                Err(anyhow!("SMTP authentication failed: {}", e))
            }

        }
    }

    /// Send a book file attachment to a recipient email address (e.g. Kindle address)
    pub fn send_book_attachment(
        &self,
        recipient_email: &str,
        book_title: &str,
        book_author: Option<&str>,
        file_name: &str,
        file_bytes: &[u8],
        extension: &str,
    ) -> Result<()> {
        info!("Sending book attachment '{}' ({}, {} bytes) via SMTP to '{}'", book_title, file_name, file_bytes.len(), recipient_email);

        if extension.eq_ignore_ascii_case("epub") {
            match check_epub_drm(file_bytes) {
                Ok(true) => {
                    return Err(anyhow!(
                        "EPUB file contains DRM encryption (Adobe ADEPT / LCP). Amazon Send-to-Kindle rejects DRM-protected EPUB files."
                    ));
                }
                Err(e) => {
                    warn!("Failed to inspect EPUB DRM status: {}", e);
                }
                Ok(false) => {}
            }
        }


        let mime_type = match extension.to_lowercase().as_str() {
            "epub" => "application/epub+zip",
            "pdf" => "application/pdf",
            "mobi" => "application/x-mobipocket-ebook",
            "azw3" => "application/vnd.amazon.mobi8-ebook",
            _ => "application/octet-stream",
        };

        let content_type = ContentType::parse(mime_type)
            .unwrap_or_else(|_| ContentType::parse("application/octet-stream").unwrap());

        let attachment = Attachment::new(file_name.to_string())
            .body(file_bytes.to_vec(), content_type);

        let from_addr = if self.config.from_email.is_empty() {
            &self.config.username
        } else {
            &self.config.from_email
        };

        let text_body = match book_author.map(|a| a.trim()).filter(|a| !a.is_empty() && *a != "Unknown") {
            Some(author) => format!("Here's the book we recommend this week: \"{}\" by {}", book_title, author),
            None => format!("Here's the book we recommend this week: \"{}\"", book_title),
        };

        let email = Message::builder()
            .from(from_addr.parse().context(format!("Invalid from_email address: '{}'", from_addr))?)
            .to(recipient_email.parse().context(format!("Invalid recipient_email address: '{}'", recipient_email))?)
            .subject("Here's your recommended book")
            .multipart(
                MultiPart::mixed()
                    .singlepart(SinglePart::plain(text_body))
                    .singlepart(attachment),
            )
            .context("Failed to build multipart email message")?;





        let transport = self.build_transport()?;
        
        if let Err(err) = transport.send(&email) {
            error!("SMTP Delivery Failed to '{}': {:?}", recipient_email, err);
            return Err(anyhow!("SMTP Delivery Failed: {}", err));
        }

        info!("Successfully delivered email for '{}' to '{}'", book_title, recipient_email);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_attachment_filename() {
        assert_eq!(
            format_attachment_filename("Dune", Some("Frank Herbert"), "epub"),
            "Dune (Frank Herbert).epub"
        );
        assert_eq!(
            format_attachment_filename("1984", None, ".pdf"),
            "1984.pdf"
        );
        assert_eq!(
            format_attachment_filename("Title: Test", Some("Author / Name"), "epub"),
            "Title_ Test (Author _ Name).epub"
        );
    }

    #[test]
    fn test_extract_epub_metadata_non_epub() {
        let dummy = b"not an epub file";
        assert!(extract_epub_metadata(dummy).is_none());
    }
}




