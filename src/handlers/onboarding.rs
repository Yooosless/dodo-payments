use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::AppState;

#[derive(Deserialize)]
pub struct OnboardPayload {
    pub name: String,
    pub webhook_url: Option<String>,
}

#[derive(Serialize)]
pub struct OnboardResponse {
    pub business_id: Uuid,
    pub business_name: String,
    pub plaintext_api_key: String,
    pub webhook_secret: String,
}

pub async fn onboard_business(
    State(state): State<AppState>,
    Json(payload): Json<OnboardPayload>,
) -> impl IntoResponse {
    if payload.name.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "Business name cannot be empty"
            })),
        )
            .into_response();
    }

    let mut token_bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut token_bytes);

    let plaintext_key = format!(
        "dodo_live_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token_bytes)
    );

    let mut hasher = Sha256::new();
    hasher.update(plaintext_key.as_bytes());

    let hashed_key = format!("{:x}", hasher.finalize());

    let mut secret_bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut secret_bytes);

    let webhook_secret = format!(
        "whsec_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret_bytes)
    );

    let business_id = Uuid::new_v4();

    let insert_res = sqlx::query(
        "INSERT INTO businesses
        (id, name, api_key_hash, webhook_secret, webhook_url)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(business_id)
    .bind(&payload.name)
    .bind(hashed_key)
    .bind(&webhook_secret)
    .bind(payload.webhook_url)
    .execute(&state.db)
    .await;

    match insert_res {
        Ok(_) => (
            StatusCode::CREATED,
            Json(OnboardResponse {
                business_id,
                business_name: payload.name,
                plaintext_api_key: plaintext_key,
                webhook_secret,
            }),
        )
            .into_response(),

        Err(e) => {
            tracing::error!("Failed to onboard business: {:?}", e);

            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Failed to complete onboarding database routine"
                })),
            )
                .into_response()
        }
    }
}