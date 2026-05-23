use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

pub mod handlers;
pub mod services;
#[cfg(test)]
mod tests;
// Shared app context containing our Postgres pool
#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::PgPool,
}

#[tokio::main]
async fn main() {
    // Initialize standard logging tracing
    tracing_subscriber::fmt::init();

    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5432/dodo".to_string());

    // Connect to the database
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&db_url)
        .await
        .expect("Failed to connect to Postgres");

    let state = AppState { db: pool };

   // Inside main() in src/main.rs, update your app Router:
        let app = Router::new()
            .route("/api/v1/protected", get(protected_handler))
            .route("/api/v1/onboard", post(handlers::onboard_business)) // Public onboarding endpoint
            .route("/api/v1/customers", post(handlers::create_customer).get(handlers::list_customers))
            .route("/api/v1/invoices", post(handlers::create_invoice))
            .route("/api/v1/invoices/:id", get(handlers::get_invoice))
            // New Payment Execution Paths
            .route("/api/v1/invoices/:id/pay", post(handlers::pay_invoice))
            .route("/mock-psp/charge", post(handlers::mock_psp_handler))
            .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    tracing::info!("Server listening on http://0.0.0.0:3000");
    axum::serve(listener, app).await.unwrap();
}

// Defining our Domain Business Model with Clone
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct AuthenticatedBusiness {
    pub id: Uuid,
    pub name: String,
}

// Implement FromRequestParts using AppState as the core state parameter
#[async_trait]
impl FromRequestParts<AppState> for AuthenticatedBusiness {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        // 1. Extract the Authorization header safely from parts
        let auth_header = parts
            .headers
            .get("Authorization")
            .and_then(|h| h.to_str().ok())
            .ok_or_else(|| {
                (StatusCode::UNAUTHORIZED, Json(json!({"error": "Missing Authorization header"}))).into_response()
            })?;

        if !auth_header.starts_with("Bearer ") {
            return Err((StatusCode::UNAUTHORIZED, Json(json!({"error": "Invalid Authorization header format"}))).into_response());
        }

        let api_key = &auth_header["Bearer ".len()..];

        // 2. Hash the raw incoming API key using SHA-256
        let mut hasher = Sha256::new();
        hasher.update(api_key.as_bytes());
        let hashed_key = format!("{:x}", hasher.finalize());

        // 3. Look up using dynamic runtime mapping against state.db
        let business = sqlx::query_as::<_, AuthenticatedBusiness>(
            "SELECT id, name FROM businesses WHERE api_key_hash = $1"
        )
        .bind(hashed_key)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| {
            tracing::error!("Database auth error: {:?}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response()
        })?
        .ok_or_else(|| {
            (StatusCode::UNAUTHORIZED, Json(json!({"error": "Invalid API key"}))).into_response()
        })?;

        Ok(business)
    }
}

// An example protected handler
async fn protected_handler(business: AuthenticatedBusiness) -> impl IntoResponse {
    Json(json!({
        "message": format!("Welcome back, {}!", business.name),
        "business_id": business.id
    }))
}