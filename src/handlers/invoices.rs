use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{AppState, AuthenticatedBusiness};
use crate::handlers::webhook::WebhookConfigRow;

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

#[derive(Serialize, FromRow)]
pub struct InvoiceResponse {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub state: String,
    pub total_amount_cents: i64,
    pub due_date: DateTime<Utc>,
}

#[derive(FromRow)]
pub struct InvoiceLockRow {
    pub id: Uuid,
    pub state: String,
    pub total_amount_cents: i64,
}

pub async fn create_invoice(
    business: AuthenticatedBusiness,
    State(state): State<AppState>,
    Json(payload): Json<CreateInvoicePayload>,
) -> impl IntoResponse {
    if payload.line_items.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "Invoice must contain at least one line item"
            })),
        )
            .into_response();
    }

    let mut total_amount_cents: i64 = 0;

    for item in &payload.line_items {
        if item.quantity <= 0 || item.unit_amount_cents <= 0 {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "Quantity and unit amount must be greater than zero"
                })),
            )
                .into_response();
        }

        total_amount_cents += item.unit_amount_cents * item.quantity as i64;
    }

    let mut tx = match state.db.begin().await {
        Ok(t) => t,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Database error"
                })),
            )
                .into_response()
        }
    };

    let customer_check = sqlx::query(
        "SELECT id
         FROM customers
         WHERE id = $1
         AND business_id = $2",
    )
    .bind(payload.customer_id)
    .bind(business.id)
    .fetch_optional(&mut *tx)
    .await;

    if let Ok(None) = customer_check {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "Customer not found or invalid"
            })),
        )
            .into_response();
    }

    let invoice_id = Uuid::new_v4();
    let initial_state = "DRAFT".to_string();

    let insert_invoice_res = sqlx::query(
        "INSERT INTO invoices
        (id, business_id, customer_id, state, total_amount_cents, due_date)
         VALUES ($1, $2, $3, $4, $5, $6)",
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

        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": "Failed to build invoice"
            })),
        )
            .into_response();
    }

    for item in payload.line_items {
        let item_res = sqlx::query(
            "INSERT INTO invoice_items
            (invoice_id, description, quantity, unit_amount_cents)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(invoice_id)
        .bind(item.description)
        .bind(item.quantity)
        .bind(item.unit_amount_cents)
        .execute(&mut *tx)
        .await;

        if let Err(e) = item_res {
            tracing::error!("Failed to insert invoice item: {:?}", e);

            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Failed to preserve line items"
                })),
            )
                .into_response();
        }
    }

    if let Err(e) = tx.commit().await {
        tracing::error!("Transaction commit failed: {:?}", e);

        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": "Failed to save invoice record"
            })),
        )
            .into_response();
    }

    let db_pool = state.db.clone();
    let biz_id = business.id;
    let inv_id = invoice_id;
    let inv_total = total_amount_cents;
    let inv_cust = payload.customer_id;

    tokio::spawn(async move {
        let biz_lookup = sqlx::query_as::<_, WebhookConfigRow>(
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
                "invoice.created".to_string(),
                json!({
                    "invoice_id": inv_id,
                    "customer_id": inv_cust,
                    "total_amount_cents": inv_total,
                    "state": "DRAFT"
                }),
            ).await;
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
    )
        .into_response()
}

pub async fn get_invoice(
    business: AuthenticatedBusiness,
    State(state): State<AppState>,
    Path(invoice_id): Path<Uuid>,
) -> impl IntoResponse {
    let invoice = sqlx::query_as::<_, InvoiceResponse>(
        "SELECT id, customer_id, state, total_amount_cents, due_date
         FROM invoices
         WHERE id = $1
         AND business_id = $2",
    )
    .bind(invoice_id)
    .bind(business.id)
    .fetch_optional(&state.db)
    .await;

    match invoice {
        Ok(Some(inv)) => Json(inv).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": "Invoice not found"
            })),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("Failed to fetch invoice: {:?}", e);

            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Database error"
                })),
            )
                .into_response()
        }
    }
}