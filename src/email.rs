use crate::config::SmtpSettings;
use anyhow::{anyhow, Context, Result};
use lettre::message::{header::ContentType, Attachment, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use tracing::{info, warn};

/// Email Sender Service using Gmail SMTP
#[derive(Clone, Debug)]
pub struct EmailSender {
    config: SmtpSettings,
}

impl EmailSender {
    pub fn new(config: SmtpSettings) -> Self {
        Self { config }
    }

    /// Build an SmtpTransport client using Gmail SMTP settings
    fn build_transport(&self) -> Result<SmtpTransport> {
        let creds = Credentials::new(self.config.username.clone(), self.config.password.clone());

        let transport = SmtpTransport::relay(&self.config.host)
            .context(format!("Failed to connect to SMTP host '{}'", self.config.host))?
            .port(self.config.port)
            .credentials(creds)
            .build();

        Ok(transport)
    }

    /// Check connection health to SMTP server
    pub fn check_connection(&self) -> Result<()> {
        info!("Verifying SMTP connection to '{}:{}'...", self.config.host, self.config.port);
        let transport = self.build_transport()?;
        if transport.test_connection().unwrap_or(false) {
            info!("SMTP server connection verified successfully");
            Ok(())
        } else {
            warn!("SMTP test connection failed");
            Err(anyhow!("SMTP server test connection failed"))
        }
    }

    /// Send a book file attachment to a recipient email address
    pub fn send_book_attachment(
        &self,
        recipient_email: &str,
        book_title: &str,
        file_name: &str,
        file_bytes: &[u8],
        extension: &str,
    ) -> Result<()> {
        info!("Sending book attachment '{}' ({}) via SMTP to '{}'", book_title, file_name, recipient_email);

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

        let text_body = format!(
            "Hello,\n\nHere is your requested book from Z-Library:\n\nTitle: {}\nFilename: {}\n\nSent automatically by Librero Daemon.",
            book_title, file_name
        );

        let email = Message::builder()
            .from(self.config.from_email.parse().context("Invalid from_email address")?)
            .to(recipient_email.parse().context("Invalid recipient_email address")?)
            .subject(format!("Book: {}", book_title))
            .multipart(
                MultiPart::mixed()
                    .singlepart(SinglePart::plain(text_body))
                    .singlepart(attachment),
            )
            .context("Failed to build multipart email message")?;

        let transport = self.build_transport()?;
        transport.send(&email).context("Failed to deliver email via SMTP transport")?;

        info!("Successfully delivered email for '{}' to '{}'", book_title, recipient_email);
        Ok(())
    }
}
