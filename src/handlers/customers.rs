use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{AppState, AuthenticatedBusiness};

#[derive(Deserialize)]
pub struct CreateCustomerPayload {
    pub name: String,
    pub email: String,
}

#[derive(Serialize, FromRow)]
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
         ON CONFLICT (business_id, email)
         DO UPDATE SET name = EXCLUDED.name
         RETURNING id, name, email",
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

            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Failed to create customer"
                })),
            )
                .into_response()
        }
    }
}

pub async fn list_customers(
    business: AuthenticatedBusiness,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let customers = sqlx::query_as::<_, CustomerResponse>(
        "SELECT id, name, email
         FROM customers
         WHERE business_id = $1
         ORDER BY created_at DESC",
    )
    .bind(business.id)
    .fetch_all(&state.db)
    .await;

    match customers {
        Ok(list) => Json(list).into_response(),
        Err(e) => {
            tracing::error!("Failed to list customers: {:?}", e);

            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Failed to fetch customers"
                })),
            )
                .into_response()
        }
    }
}