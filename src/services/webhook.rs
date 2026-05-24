use hmac::{Hmac, Mac};
use sha2::Sha256;
use serde::Serialize;
use uuid::Uuid;
use chrono::Utc;

type HmacSha256 = Hmac<Sha256>;

#[derive(Serialize, Clone)]
pub struct WebhookPayload {
    pub id: Uuid,
    pub event: String,
    pub created_at: i64,
    pub data: serde_json::Value,
}

pub async fn dispatch_webhook(
    webhook_url: Option<String>,
    webhook_secret: String,
    event_name: String,
    event_data: serde_json::Value,
) {
    let url = match webhook_url {
        Some(u) => u,
        None => {
            tracing::info!("Skipping webhook dispatch: No webhook_url configured for business");
            return;
        }
    };

    let payload = WebhookPayload {
        id: Uuid::new_v4(),
        event: event_name,
        created_at: Utc::now().timestamp(),
        data: event_data,
    };

    let serialized_payload = serde_json::to_string(&payload).unwrap_or_default();

    // Generate HMAC Signature
    let mut mac = HmacSha256::new_from_slice(webhook_secret.as_bytes())
        .expect("HMAC secret compilation error");
    mac.update(serialized_payload.as_bytes());
    let computed_signature = format!("{:x}", mac.finalize().into_bytes());

    let client = reqwest::Client::new();
    let mut attempts = 0;
    let max_attempts = 5;
    let mut retry_delay_secs = 2;

    // Retry Loop
    while attempts < max_attempts {
        tracing::info!(
            "Dispatching webhook event '{}' to {} (Attempt {}/{})",
            payload.event, url, attempts + 1, max_attempts
        );

        let response = client.post(&url)
            .header("Content-Type", "application/json")
            .header("X-Dodo-Signature", &computed_signature)
            .header("X-Dodo-Timestamp", payload.created_at.to_string())
            .body(serialized_payload.clone())
            .send()
            .await;

        match response {
            Ok(res) if res.status().is_success() => {
                tracing::info!("Webhook '{}' successfully delivered to {}", payload.event, url);
                return;
            }
            Ok(res) => {
                tracing::warn!("Webhook server rejected delivery with status code: {}", res.status());
            }
            Err(err) => {
                tracing::error!("Webhook transport error encountered: {:?}", err);
            }
        }

        attempts += 1;
        if attempts < max_attempts {
            tokio::time::sleep(tokio::time::Duration::from_secs(retry_delay_secs)).await;
            retry_delay_secs *= 2;
        }
    }

    tracing::error!(
        "CRITICAL: Webhook event '{}' exhausted all retry budgets.",
        payload.event
    );
}