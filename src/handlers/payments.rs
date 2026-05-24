use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{AppState, AuthenticatedBusiness};
use crate::handlers::invoices::InvoiceLockRow;
use crate::handlers::webhook::WebhookConfigRow;

#[derive(Deserialize, Serialize, Clone)]
pub struct PayInvoicePayload {
    pub card_token: String,
}

#[derive(Serialize)]
pub struct PaymentSuccessResponse {
    pub invoice_id: Uuid,
    pub status: String,
    pub amount_cents: i64,
    pub psp_reference: Option<Uuid>,
}

#[derive(Deserialize, Serialize)]
pub struct PspRequest {
    pub card_token: String,
    pub amount_cents: i64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PspResponse {
    pub status: String,
    pub psp_ref: Option<Uuid>,
    pub code: Option<String>,
}

#[derive(FromRow)]
struct IdempotencyRow {
    request_hash: String,
    response_status: Option<i32>,
    response_body: Option<String>,
}

pub async fn pay_invoice(
    business: AuthenticatedBusiness,
    State(state): State<AppState>,
    Path(invoice_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<PayInvoicePayload>,
) -> impl IntoResponse {
    let idempotency_key = match headers
        .get("Idempotency-Key")
        .and_then(|h| h.to_str().ok())
    {
        Some(k) => k.to_string(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "Idempotency-Key header is required"
                })),
            )
                .into_response()
        }
    };

    let serialized_body =
        serde_json::to_string(&payload).unwrap_or_default();

    let current_request_hash =
        hash_request_body(&serialized_body);

    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Database concurrency failure"
                })),
            )
                .into_response()
        }
    };

    let existing_key = sqlx::query_as::<_, IdempotencyRow>(
        "SELECT request_hash, response_status, response_body
         FROM idempotency_keys
         WHERE idempotency_key = $1
         AND business_id = $2",
    )
    .bind(&idempotency_key)
    .bind(business.id)
    .fetch_optional(&mut *tx)
    .await;

    if let Ok(Some(row)) = existing_key {
        if row.request_hash != current_request_hash {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "Idempotency body changed"
                })),
            )
                .into_response();
        }

        if let (Some(status), Some(body)) =
            (row.response_status, row.response_body)
        {
            let parsed_json: serde_json::Value =
                serde_json::from_str(&body)
                    .unwrap_or(json!({}));

            return (
                StatusCode::from_u16(status as u16)
                    .unwrap_or(StatusCode::OK),
                Json(parsed_json),
            )
                .into_response();
        } else {
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "Payment processing cycle active"
                })),
            )
                .into_response();
        }
    }

    let register_key = sqlx::query(
        "INSERT INTO idempotency_keys
        (idempotency_key, business_id, request_path, request_hash)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(&idempotency_key)
    .bind(business.id)
    .bind(format!(
        "/api/v1/invoices/{}/pay",
        invoice_id
    ))
    .bind(&current_request_hash)
    .execute(&mut *tx)
    .await;

    if register_key.is_err() {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "Concurrent execution conflict"
            })),
        )
            .into_response();
    }

    let invoice_lock =
        sqlx::query_as::<_, InvoiceLockRow>(
            "SELECT id, state, total_amount_cents
             FROM invoices
             WHERE id = $1
             AND business_id = $2
             FOR UPDATE",
        )
        .bind(invoice_id)
        .bind(business.id)
        .fetch_optional(&mut *tx)
        .await;

    let target_invoice = match invoice_lock {
        Ok(Some(inv)) => inv,
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "error": "Invoice not found"
                })),
            )
                .into_response()
        }
    };

    if target_invoice.state == "PAID" {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "error": "Invoice already PAID"
            })),
        )
            .into_response();
    }

    if target_invoice.state == "PROCESSING" {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({
                "error": "Invoice is already processing"
            })),
        )
            .into_response();
    }

    let _ = sqlx::query(
        "UPDATE invoices
         SET state = 'PROCESSING'
         WHERE id = $1",
    )
    .bind(invoice_id)
    .execute(&mut *tx)
    .await;

    if tx.commit().await.is_err() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": "Database lock commit failure"
            })),
        )
            .into_response();
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .unwrap_or_default();

    let psp_payload = PspRequest {
        card_token: payload.card_token.clone(),
        amount_cents: target_invoice.total_amount_cents,
    };

    let psp_call = client
        .post("http://127.0.0.1:3000/mock-psp/charge")
        .json(&psp_payload)
        .send()
        .await;

    let mut post_tx = match state.db.begin().await {
        Ok(t) => t,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error":
                    "Database commit transaction setup failure"
                })),
            )
                .into_response()
        }
    };

    let final_invoice_state;
    let final_attempt_status;
    let mut psp_ref: Option<Uuid> = None;
    let mut error_code: Option<String> = None;
    let mut api_status = StatusCode::OK;
    let response_payload: serde_json::Value;

    match psp_call {
        Ok(response) => {
            let status_code = response.status();

            let parsed_psp: Result<PspResponse, _> =
                response.json().await;

            if status_code.is_success()
                && parsed_psp.is_ok()
            {
                let psp_data = parsed_psp.unwrap();

                final_invoice_state = "PAID".to_string();
                final_attempt_status =
                    "success".to_string();

                psp_ref = psp_data.psp_ref;

                response_payload =
                    json!(PaymentSuccessResponse {
                        invoice_id,
                        status: "succeeded".to_string(),
                        amount_cents:
                            target_invoice.total_amount_cents,
                        psp_reference: psp_ref,
                    });
            } else {
                let psp_data = parsed_psp.unwrap_or(
                    PspResponse {
                        status: "failed".to_string(),
                        psp_ref: None,
                        code: Some(
                            "declined".to_string()
                        ),
                    },
                );

                final_invoice_state =
                    "OPEN".to_string();

                final_attempt_status =
                    "failed".to_string();

                error_code = psp_data.code;

                api_status = StatusCode::BAD_REQUEST;

                response_payload = json!({
                    "error": "Payment failed",
                    "code": error_code
                        .clone()
                        .unwrap_or_else(|| {
                            "declined".to_string()
                        })
                });
            }
        }
        Err(err) => {
            tracing::error!("PSP Gateway Timeout/Error: {:?}", err);

            final_invoice_state = "OPEN".to_string();
            final_attempt_status = "failed".to_string();
            error_code = Some("gateway_timeout".to_string());

            api_status = StatusCode::REQUEST_TIMEOUT;

            response_payload = json!({
                "error": "Payment gateway timeout",
                "invoice_id": invoice_id,
                "state": "OPEN"
            });
        }
    }

    let _ = sqlx::query(
        "INSERT INTO payment_attempts
        (invoice_id, status, card_token,
         psp_reference, failure_code, amount_cents)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(invoice_id)
    .bind(&final_attempt_status)
    .bind(&payload.card_token)
    .bind(psp_ref)
    .bind(&error_code)
    .bind(target_invoice.total_amount_cents)
    .execute(&mut *post_tx)
    .await;

    let _ = sqlx::query(
        "UPDATE invoices
         SET state = $1
         WHERE id = $2",
    )
    .bind(&final_invoice_state)
    .bind(invoice_id)
    .execute(&mut *post_tx)
    .await;

    let response_string =
        serde_json::to_string(&response_payload)
            .unwrap_or_default();

    let _ = sqlx::query(
        "UPDATE idempotency_keys
         SET response_status = $1,
             response_body = $2
         WHERE idempotency_key = $3
         AND business_id = $4",
    )
    .bind(api_status.as_u16() as i32)
    .bind(&response_string)
    .bind(&idempotency_key)
    .bind(business.id)
    .execute(&mut *post_tx)
    .await;

    let db_pool = state.db.clone();

    let biz_id = business.id;

    let webhook_event =
        if final_invoice_state == "PAID" {
            "invoice.paid".to_string()
        } else {
            "invoice.payment_failed".to_string()
        };

    let webhook_data = json!({
        "invoice_id": invoice_id,
        "state": final_invoice_state,
        "amount_cents":
            target_invoice.total_amount_cents,
        "psp_reference": psp_ref
    });

    tokio::spawn(async move {
        let biz_lookup =
            sqlx::query_as::<_, WebhookConfigRow>(
                "SELECT webhook_url, webhook_secret
                 FROM businesses
                 WHERE id = $1",
            )
            .bind(biz_id)
            .fetch_optional(&db_pool)
            .await;

        if let Ok(Some(row)) = biz_lookup {
            crate::services::webhook::dispatch_webhook(
                row.webhook_url,
                row.webhook_secret,
                webhook_event,
                webhook_data,
            );
        }
    });

    let _ = post_tx.commit().await;

    (
        api_status,
        Json(response_payload),
    )
        .into_response()
}

fn hash_request_body(body: &str) -> String {
    let mut hasher = Sha256::new();

    hasher.update(body.as_bytes());

    format!("{:x}", hasher.finalize())
}