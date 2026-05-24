use axum::{
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use serde_json::json;
use uuid::Uuid;

use crate::handlers::payments::PspRequest;

pub async fn mock_psp_handler(
    Json(payload): Json<PspRequest>,
) -> impl IntoResponse {
    match payload.card_token.as_str() {
        "tok_success" => {
            tokio::time::sleep(
        tokio::time::Duration::from_secs(29), // 300 seconds = 5 minutes
    )
    .await;
            (
                StatusCode::OK,
                Json(json!({
                    "status": "succeeded",
                    "psp_ref": Uuid::new_v4()
                })),
            )
                .into_response()
        }

        "tok_insufficient_funds" => {
            tokio::time::sleep(
                tokio::time::Duration::from_millis(100),
            )
            .await;

            (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "failed",
                    "code": "insufficient_funds"
                })),
            )
                .into_response()
        }

        "tok_card_declined" => {
            tokio::time::sleep(
                tokio::time::Duration::from_millis(100),
            )
            .await;

            (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "status": "failed",
                    "code": "card_declined"
                })),
            )
                .into_response()
        }

        "tok_timeout" => {
            tokio::time::sleep(
                tokio::time::Duration::from_millis(100),
            )
            .await;

            (
                StatusCode::OK,
                Json(json!({
                    "status": "succeeded",
                    "psp_ref": Uuid::new_v4()
                })),
            )
                .into_response()
        }

        "tok_network_error" | _ => {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Connection Reset By Peer",
            )
                .into_response()
        }
    }
}