#[cfg(test)]
mod integration_tests {
    use axum::{body::Body, http::{Request, StatusCode}, response::Response};
    use tower::util::ServiceExt;
    use serde_json::json;
    use uuid::Uuid;
    use sha2::{Digest, Sha256};
    use futures::future::join_all;
    use crate::{AppState, handlers};

    // Helper to initialize database links and the app router
    async fn setup_test_context() -> (sqlx::PgPool, axum::Router) {
        let db_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgres://postgres:supersecretpassword@localhost:5432/dodo".to_string());
        
        let pool = sqlx::PgPool::connect(&db_url)
            .await
            .expect("Test failed: Could not connect to local test database");

        let state = AppState { db: pool.clone() };
        
        let app = axum::Router::new()
            .route("/api/v1/invoices/:id/pay", axum::routing::post(handlers::pay_invoice))
            .route("/mock-psp/charge", axum::routing::post(handlers::mock_psp_handler))
            .with_state(state);

        (pool, app)
    }

    // Helper to dynamically seed a distinct business per test to eliminate parallel database collisions
    async fn seed_unique_business(pool: &sqlx::PgPool, raw_key: &str) -> Uuid {
        let mut hasher = Sha256::new();
        hasher.update(raw_key.as_bytes());
        let hashed_key = format!("{:x}", hasher.finalize());

        let biz_id = Uuid::new_v4();
        sqlx::query!(
            "INSERT INTO businesses (id, name, api_key_hash, webhook_secret) VALUES ($1, $2, $3, $4)",
            biz_id, format!("Biz-{}", biz_id), hashed_key, "whsec_test_secret"
        )
        .execute(pool)
        .await
        .unwrap();

        biz_id
    }

    // ==========================================
    // TEST 1: N-Concurrent Payments Race Condition Test
    // ==========================================
    #[tokio::test]
    async fn test_concurrent_payment_race_condition() {
        let (pool, app) = setup_test_context().await;
        
        // Append a random UUID to ensure string uniqueness across multiple test scopes
        let raw_key = format!("test_key_concurrency_{}", Uuid::new_v4());
        let biz_id = seed_unique_business(&pool, &raw_key).await;

        let cust_id = Uuid::new_v4();
        let invoice_id = Uuid::new_v4();

        sqlx::query!("INSERT INTO customers (id, business_id, name, email) VALUES ($1, $2, $3, $4)", cust_id, biz_id, "Race Condition Customer", "race@example.com").execute(&pool).await.unwrap();
        sqlx::query!("INSERT INTO invoices (id, business_id, customer_id, state, total_amount_cents, due_date) VALUES ($1, $2, $3, $4, $5, NOW())", invoice_id, biz_id, cust_id, "OPEN", 25000).execute(&pool).await.unwrap();

        let num_requests = 5;
        let mut tasks = Vec::new();

        for i in 0..num_requests {
            let app_clone = app.clone();
            let uri = format!("/api/v1/invoices/{}/pay", invoice_id);
            let payload = json!({ "card_token": "tok_success" }).to_string();
            
            let req = Request::builder()
                .method("POST")
                .uri(uri)
                .header("Authorization", format!("Bearer {}", raw_key))
                .header("Idempotency-Key", format!("race_key_{}_{}", i, Uuid::new_v4()))
                .header("Content-Type", "application/json")
                .body(Body::from(payload))
                .unwrap();

            tasks.push(app_clone.oneshot(req));
        }

        let responses = join_all(tasks).await;

        let mut success_count = 0;
        let mut rejected_count = 0;

        for res in responses {
            let unwrapped_response: Response = res.unwrap();
            let status = unwrapped_response.status();
            
            if status == StatusCode::OK {
                success_count += 1;
            } else if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::CONFLICT {
                rejected_count += 1;
            }
        }

        assert_eq!(success_count, 1, "CRITICAL FAILURE: Double charge occurred.");
        assert_eq!(rejected_count, num_requests - 1);

        let final_state: String = sqlx::query_scalar!("SELECT state FROM invoices WHERE id = $1", invoice_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(final_state, "PAID");
    }

    // ==========================================
    // TEST 2: Idempotency Key Shield Test
    // ==========================================
    #[tokio::test]
    async fn test_payment_idempotency_shield() {
        let (pool, app) = setup_test_context().await;
        
        // Append a random UUID to ensure string uniqueness across multiple test scopes
        let raw_key = format!("test_key_idemp_{}", Uuid::new_v4());
        let biz_id = seed_unique_business(&pool, &raw_key).await;
        
        let cust_id = Uuid::new_v4();
        let invoice_id = Uuid::new_v4();

        sqlx::query!("INSERT INTO customers (id, business_id, name, email) VALUES ($1, $2, $3, $4)", cust_id, biz_id, "Idemp Customer", "idemp@example.com").execute(&pool).await.unwrap();
        sqlx::query!("INSERT INTO invoices (id, business_id, customer_id, state, total_amount_cents, due_date) VALUES ($1, $2, $3, $4, $5, NOW())", invoice_id, biz_id, cust_id, "OPEN", 15000).execute(&pool).await.unwrap();

        let payload = json!({ "card_token": "tok_success" }).to_string();
        let unique_idemp_header = format!("idemp_hdr_{}", Uuid::new_v4());

        // Call 1: Initial execution
        let req1 = Request::builder()
            .method("POST")
            .uri(format!("/api/v1/invoices/{}/pay", invoice_id))
            .header("Authorization", format!("Bearer {}", raw_key))
            .header("Idempotency-Key", &unique_idemp_header)
            .header("Content-Type", "application/json")
            .body(Body::from(payload.clone()))
            .unwrap();
        
        let response1 = app.clone().oneshot(req1).await.unwrap();
        assert_eq!(response1.status(), StatusCode::OK);

        // Call 2: Retry execution
        let req2 = Request::builder()
            .method("POST")
            .uri(format!("/api/v1/invoices/{}/pay", invoice_id))
            .header("Authorization", format!("Bearer {}", raw_key))
            .header("Idempotency-Key", &unique_idemp_header)
            .header("Content-Type", "application/json")
            .body(Body::from(payload))
            .unwrap();

        let response2 = app.oneshot(req2).await.unwrap();
        assert_eq!(response2.status(), StatusCode::OK);
    }

    // ==========================================
    // TEST 3: Gateway Failure/Timeout Resilience Test
    // ==========================================
    #[tokio::test]
    async fn test_psp_timeout_graceful_handling() {
        let (pool, app) = setup_test_context().await;
        
        // Append a random UUID to ensure string uniqueness across multiple test scopes
        let raw_key = format!("test_key_timeout_{}", Uuid::new_v4());
        let biz_id = seed_unique_business(&pool, &raw_key).await;

        let cust_id = Uuid::new_v4();
        let invoice_id = Uuid::new_v4();

        sqlx::query!("INSERT INTO customers (id, business_id, name, email) VALUES ($1, $2, $3, $4)", cust_id, biz_id, "Timeout Customer", "timeout@example.com").execute(&pool).await.unwrap();
        sqlx::query!("INSERT INTO invoices (id, business_id, customer_id, state, total_amount_cents, due_date) VALUES ($1, $2, $3, $4, $5, NOW())", invoice_id, biz_id, cust_id, "OPEN", 5000).execute(&pool).await.unwrap();

        let payload = json!({ "card_token": "tok_timeout" }).to_string();
        let unique_timeout_header = format!("timeout_hdr_{}", Uuid::new_v4());

        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/v1/invoices/{}/pay", invoice_id))
            .header("Authorization", format!("Bearer {}", raw_key))
            .header("Idempotency-Key", &unique_timeout_header)
            .header("Content-Type", "application/json")
            .body(Body::from(payload))
            .unwrap();

        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        let final_state: String = sqlx::query_scalar!("SELECT state FROM invoices WHERE id = $1", invoice_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(final_state, "PROCESSING");
    }
}