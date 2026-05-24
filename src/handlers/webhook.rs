use sqlx::FromRow;

#[derive(FromRow)]
pub struct WebhookConfigRow {
    pub webhook_url: Option<String>,
    pub webhook_secret: String,
}