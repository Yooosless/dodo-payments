use axum::{
    extract::{Path, State},
    http::{StatusCode, HeaderMap},
    response::IntoResponse,
    Json,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;
use chrono::{DateTime, Utc};
use crate::{AppState, AuthenticatedBusiness};
use sha2::Digest; // Brings the crypto functions into scope
// --- PAYLOADS & DATA FOR CUSTOMERS ---
#[derive(Deserialize)]
pub struct CreateCustomerPayload {
    pub name: String,
    pub email: String,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct CustomerResponse {
    pub id: Uuid,
    pub name: String,
    pub email: String,
}

pub async fn create_customer(
    business: AuthenticatedBusiness,
    State(state): State<AppState>,
    Json(payload): Json<CreateCustomerPayload>,
) -> impl IntoResponse {
    let customer = sqlx::query_as::<_, CustomerResponse>(
        "INSERT INTO customers (business_id, name, email) 
         VALUES ($1, $2, $3) 
         ON CONFLICT (business_id, email) DO UPDATE SET name = EXCLUDED.name
         RETURNING id, name, email"
    )
    .bind(business.id)
    .bind(payload.name)
    .bind(payload.email)
    .fetch_one(&state.db)
    .await;

    match customer {
        Ok(c) => (StatusCode::CREATED, Json(c)).into_response(),
        Err(e) => {
            tracing::error!("Failed to create customer: {:?}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "Failed to create customer"}))).into_response()
        }
    }
}

pub async fn list_customers(
    business: AuthenticatedBusiness,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let customers = sqlx::query_as::<_, CustomerResponse>(
        "SELECT id, name, email FROM customers WHERE business_id = $1 ORDER BY created_at DESC"
    )
    .bind(business.id)
    .fetch_all(&state.db)
    .await;

    match customers {
        Ok(list) => Json(list).into_response(),
        Err(e) => {
            tracing::error!("Failed to list customers: {:?}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "Failed to fetch customers"}))).into_response()
        }
    }
}

// --- PAYLOADS & DATA FOR INVOICES ---
#[derive(Deserialize)]
pub struct LineItemInput {
    pub description: String,
    pub quantity: i32,
    pub unit_amount_cents: i64,
}

#[derive(Deserialize)]
pub struct CreateInvoicePayload {
    pub customer_id: Uuid,
    pub due_date: DateTime<Utc>,
    pub line_items: Vec<LineItemInput>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct InvoiceResponse {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub state: String,
    pub total_amount_cents: i64,
    pub due_date: DateTime<Utc>,
}

pub async fn create_invoice(
    business: AuthenticatedBusiness,
    State(state): State<AppState>,
    Json(payload): Json<CreateInvoicePayload>,
) -> impl IntoResponse {
    if payload.line_items.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Invoice must contain at least one line item"}))).into_response();
    }

    let mut total_amount_cents: i64 = 0;
    for item in &payload.line_items {
        if item.quantity <= 0 || item.unit_amount_cents <= 0 {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "Quantity and unit amount must be greater than zero"}))).into_response();
        }
        total_amount_cents += item.unit_amount_cents * (item.quantity as i64);
    }

    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "Database error"}))).into_response(),
    };

    let customer_check = sqlx::query("SELECT id FROM customers WHERE id = $1 AND business_id = $2")
        .bind(payload.customer_id)
        .bind(business.id)
        .fetch_optional(&mut *tx)
        .await;

    if let Ok(None) = customer_check {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Customer not found or invalid"}))).into_response();
    }

    let invoice_id = Uuid::new_v4();
    let initial_state = "DRAFT".to_string();

    let insert_invoice_res = sqlx::query(
        "INSERT INTO invoices (id, business_id, customer_id, state, total_amount_cents, due_date)
         VALUES ($1, $2, $3, $4, $5, $6)"
    )
    .bind(invoice_id)
    .bind(business.id)
    .bind(payload.customer_id)
    .bind(&initial_state)
    .bind(total_amount_cents)
    .bind(payload.due_date)
    .execute(&mut *tx)
    .await;

    if let Err(e) = insert_invoice_res {
        tracing::error!("Failed to insert invoice: {:?}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "Failed to build invoice"}))).into_response();
    }

    for item in payload.line_items {
        let item_res = sqlx::query(
            "INSERT INTO invoice_items (invoice_id, description, quantity, unit_amount_cents)
             VALUES ($1, $2, $3, $4)"
        )
        .bind(invoice_id)
        .bind(item.description)
        .bind(item.quantity)
        .bind(item.unit_amount_cents)
        .execute(&mut *tx)
        .await;

        if let Err(e) = item_res {
            tracing::error!("Failed to insert invoice item: {:?}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "Failed to preserve line items"}))).into_response();
        }
    }

    if let Err(e) = tx.commit().await {
        tracing::error!("Transaction commit failed: {:?}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "Failed to save invoice record"}))).into_response();
    }

    // --- TRIGGER 1: invoice.created WEBHOOK DISPATCH ---
    let db_pool = state.db.clone();
    let biz_id = business.id;
    let inv_id = invoice_id;
    let inv_total = total_amount_cents;
    let inv_cust = payload.customer_id;

    tokio::spawn(async move {
        let biz_lookup = sqlx::query_as::<_, WebhookConfigRow>(
            "SELECT webhook_url, webhook_secret FROM businesses WHERE id = $1"
        )
        .bind(biz_id)
        .fetch_optional(&db_pool)
        .await;

        if let Ok(Some(row)) = biz_lookup {
            crate::services::webhook::dispatch_webhook(
                row.webhook_url,
                row.webhook_secret,
                "invoice.created".to_string(),
                json!({ "invoice_id": inv_id, "customer_id": inv_cust, "total_amount_cents": inv_total, "state": "DRAFT" }),
            );
        }
    });

    (
        StatusCode::CREATED,
        Json(InvoiceResponse {
            id: invoice_id,
            customer_id: payload.customer_id,
            state: initial_state,
            total_amount_cents,
            due_date: payload.due_date,
        }),
    ).into_response()
}

pub async fn get_invoice(
    business: AuthenticatedBusiness,
    State(state): State<AppState>,
    Path(invoice_id): Path<Uuid>,
) -> impl IntoResponse {
    let invoice = sqlx::query_as::<_, InvoiceResponse>(
        "SELECT id, customer_id, state, total_amount_cents, due_date 
         FROM invoices WHERE id = $1 AND business_id = $2"
    )
    .bind(invoice_id)
    .bind(business.id)
    .fetch_optional(&state.db)
    .await;

    match invoice {
        Ok(Some(inv)) => Json(inv).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({"error": "Invoice not found"}))).into_response(),
        Err(e) => {
            tracing::error!("Failed to fetch invoice: {:?}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "Database error"}))).into_response()
        }
    }
}

// --- PAYLOADS & ENGINE FOR PAYMENTS ---
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

#[derive(sqlx::FromRow)]
struct IdempotencyRow {
    request_hash: String,
    response_status: Option<i32>,
    response_body: Option<String>,
}

#[derive(sqlx::FromRow)]
pub struct InvoiceLockRow {
    pub id: Uuid,
    pub state: String,
    pub total_amount_cents: i64,
}

#[derive(sqlx::FromRow)]
struct WebhookConfigRow {
    webhook_url: Option<String>,
    webhook_secret: String,
}

pub async fn mock_psp_handler(Json(payload): Json<PspRequest>) -> impl IntoResponse {
    match payload.card_token.as_str() {
        "tok_success" => {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            (StatusCode::OK, Json(json!({ "status": "succeeded", "psp_ref": Uuid::new_v4() }))).into_response()
        }
        "tok_insufficient_funds" => {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            (StatusCode::BAD_REQUEST, Json(json!({ "status": "failed", "code": "insufficient_funds" }))).into_response()
        }
        "tok_card_declined" => {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            (StatusCode::BAD_REQUEST, Json(json!({ "status": "failed", "code": "card_declined" }))).into_response()
        }
        "tok_timeout" => {
            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
            (StatusCode::OK, Json(json!({ "status": "succeeded", "psp_ref": Uuid::new_v4() }))).into_response()
        }
        "tok_network_error" | _ => {
            (StatusCode::INTERNAL_SERVER_ERROR, "Connection Reset By Peer").into_response()
        }
    }
}

pub async fn pay_invoice(
    business: AuthenticatedBusiness,
    State(state): State<AppState>,
    Path(invoice_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<PayInvoicePayload>,
) -> impl IntoResponse {
    let idempotency_key = match headers.get("Idempotency-Key").and_then(|h| h.to_str().ok()) {
        Some(k) => k.to_string(),
        None => return (StatusCode::BAD_REQUEST, Json(json!({"error": "Idempotency-Key header is required"}))).into_response(),
    };

    let serialized_body = serde_json::to_string(&payload).unwrap_or_default();
    let current_request_hash = hash_request_body(&serialized_body);

    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "Database concurrency failure"}))).into_response(),
    };

    let existing_key = sqlx::query_as::<_, IdempotencyRow>(
        "SELECT request_hash, response_status, response_body FROM idempotency_keys WHERE idempotency_key = $1 AND business_id = $2"
    )
    .bind(&idempotency_key)
    .bind(business.id)
    .fetch_optional(&mut *tx)
    .await;

    if let Ok(Some(row)) = existing_key {
        if row.request_hash != current_request_hash {
            return (StatusCode::BAD_REQUEST, Json(json!({"error": "Idempotency body changed"}))).into_response();
        }
        if let (Some(status), Some(body)) = (row.response_status, row.response_body) {
            let parsed_json: serde_json::Value = serde_json::from_str(&body).unwrap_or(json!({}));
            return (StatusCode::from_u16(status as u16).unwrap_or(StatusCode::OK), Json(parsed_json)).into_response();
        } else {
            return (StatusCode::CONFLICT, Json(json!({"error": "Payment processing cycle active"}))).into_response();
        }
    }

    let register_key = sqlx::query(
        "INSERT INTO idempotency_keys (idempotency_key, business_id, request_path, request_hash) VALUES ($1, $2, $3, $4)"
    )
    .bind(&idempotency_key)
    .bind(business.id)
    .bind(format!("/api/v1/invoices/{}/pay", invoice_id))
    .bind(&current_request_hash)
    .execute(&mut *tx)
    .await;

    if register_key.is_err() {
        return (StatusCode::CONFLICT, Json(json!({"error": "Concurrent execution conflict"}))).into_response();
    }

    let invoice_lock = sqlx::query_as::<_, InvoiceLockRow>(
        "SELECT id, state, total_amount_cents FROM invoices WHERE id = $1 AND business_id = $2 FOR UPDATE"
    )
    .bind(invoice_id)
    .bind(business.id)
    .fetch_optional(&mut *tx)
    .await;

    let target_invoice = match invoice_lock {
        Ok(Some(inv)) => inv,
        _ => return (StatusCode::NOT_FOUND, Json(json!({"error": "Invoice not found"}))).into_response(),
    };

    if target_invoice.state == "PAID" {
        return (StatusCode::UNPROCESSABLE_ENTITY, Json(json!({"error": "Invoice already PAID"}))).into_response();
    }
    if target_invoice.state == "PROCESSING" {
        return (StatusCode::TOO_MANY_REQUESTS, Json(json!({"error": "Invoice is already processing"}))).into_response();
    }

    let _ = sqlx::query("UPDATE invoices SET state = 'PROCESSING' WHERE id = $1")
        .bind(invoice_id)
        .execute(&mut *tx)
        .await;

    if tx.commit().await.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "Database lock commit failure"}))).into_response();
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    let psp_payload = PspRequest {
        card_token: payload.card_token.clone(),
        amount_cents: target_invoice.total_amount_cents,
    };

    let psp_call = client.post("http://127.0.0.1:3000/mock-psp/charge")
        .json(&psp_payload)
        .send()
        .await;

    let mut post_tx = match state.db.begin().await {
        Ok(t) => t,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "Database commit transaction setup failure"}))).into_response(),
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
            let parsed_psp: Result<PspResponse, _> = response.json().await;

            if status_code.is_success() && parsed_psp.is_ok() {
                let psp_data = parsed_psp.unwrap();
                final_invoice_state = "PAID".to_string();
                final_attempt_status = "success".to_string();
                psp_ref = psp_data.psp_ref;
                
                response_payload = json!(PaymentSuccessResponse {
                    invoice_id,
                    status: "succeeded".to_string(),
                    amount_cents: target_invoice.total_amount_cents,
                    psp_reference: psp_ref,
                });
            } else {
                let psp_data = parsed_psp.unwrap_or(PspResponse { status: "failed".to_string(), psp_ref: None, code: Some("declined".to_string()) });
                final_invoice_state = "OPEN".to_string();
                final_attempt_status = "failed".to_string();
                error_code = psp_data.code;
                api_status = StatusCode::BAD_REQUEST;

                response_payload = json!({
                    "error": "Payment failed",
                    "code": error_code.clone().unwrap_or_else(|| "declined".to_string())
                });
            }
        }
        Err(err) => {
            tracing::error!("PSP Gateway Issue: {:?}", err);
            final_invoice_state = "PROCESSING".to_string();
            final_attempt_status = "pending".to_string();
            api_status = StatusCode::ACCEPTED;

            response_payload = json!({
                "message": "Payment pending gateway verification",
                "invoice_id": invoice_id,
                "state": "PROCESSING"
            });
        }
    }

    let _ = sqlx::query(
        "INSERT INTO payment_attempts (invoice_id, status, card_token, psp_reference, failure_code, amount_cents) VALUES ($1, $2, $3, $4, $5, $6)"
    )
    .bind(invoice_id)
    .bind(&final_attempt_status)
    .bind(&payload.card_token)
    .bind(psp_ref)
    .bind(&error_code)
    .bind(target_invoice.total_amount_cents)
    .execute(&mut *post_tx)
    .await;

    let _ = sqlx::query("UPDATE invoices SET state = $1 WHERE id = $2")
        .bind(&final_invoice_state)
        .bind(invoice_id)
        .execute(&mut *post_tx)
        .await;

    let response_string = serde_json::to_string(&response_payload).unwrap_or_default();
    let _ = sqlx::query(
        "UPDATE idempotency_keys SET response_status = $1, response_body = $2 WHERE idempotency_key = $3 AND business_id = $4"
    )
    .bind(api_status.as_u16() as i32)
    .bind(&response_string)
    .bind(&idempotency_key)
    .bind(business.id)
    .execute(&mut *post_tx)
    .await;

    // --- TRIGGER 2 & 3: invoice.paid & invoice.payment_failed WEBHOOK DISPATCH ---
    let db_pool = state.db.clone();
    let biz_id = business.id;
    let webhook_event = if final_invoice_state == "PAID" { "invoice.paid".to_string() } else { "invoice.payment_failed".to_string() };
    let webhook_data = json!({ "invoice_id": invoice_id, "state": final_invoice_state, "amount_cents": target_invoice.total_amount_cents, "psp_reference": psp_ref });

    tokio::spawn(async move {
        let biz_lookup = sqlx::query_as::<_, WebhookConfigRow>(
            "SELECT webhook_url, webhook_secret FROM businesses WHERE id = $1"
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

    (api_status, Json(response_payload)).into_response()
}

fn hash_request_body(body: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    format!("{:x}", hasher.finalize())
}

use rand::RngCore;

#[derive(Deserialize)]
pub struct OnboardPayload {
    pub name: String,
    pub webhook_url: Option<String>,
}

#[derive(Serialize)]
pub struct OnboardResponse {
    pub business_id: Uuid,
    pub business_name: String,
    pub plaintext_api_key: String, // Only shown once to the client here!
    pub webhook_secret: String,
}

// Handler: POST /api/v1/onboard
pub async fn onboard_business(
    State(state): State<AppState>,
    Json(payload): Json<OnboardPayload>,
) -> impl IntoResponse {
    if payload.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({"error": "Business name cannot be empty"}))).into_response();
    }

    // 1. Generate 24 secure random bytes for the raw API key
    let mut token_bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut token_bytes);
    let plaintext_key = format!("dodo_live_{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token_bytes));

    // 2. Hash that key instantly using SHA-256 for secure DB storage
    let mut hasher = sha2::Sha256::new();
    hasher.update(plaintext_key.as_bytes());
    let hashed_key = format!("{:x}", hasher.finalize());

    // 3. Generate a secure random webhook secret signing key
    let mut secret_bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut secret_bytes);
    let webhook_secret = format!("whsec_{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret_bytes));

    let business_id = Uuid::new_v4();

    // 4. Persist the business into PostgreSQL
    let insert_res = sqlx::query(
        "INSERT INTO businesses (id, name, api_key_hash, webhook_secret, webhook_url) VALUES ($1, $2, $3, $4, $5)"
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
        ).into_response(),
        Err(e) => {
            tracing::error!("Failed to onboard business: {:?}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "Failed to complete onboarding database routine"}))).into_response()
        }
    }
}