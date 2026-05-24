# Dodo Payments Backend Engine

A payment and billing engine built with Rust and PostgreSQL that implements strict financial constraints, request deduplication, and safe transaction state tracking.

---

## 1. System Requirements

Ensure you have the following tools installed locally before spinning up the application:
* **Docker & Docker Compose**: Handles the multi-container setup (Rust app server + PostgreSQL db).
* **Rust Toolchain (Stable)**: Optional, only required if you intend to run cargo commands directly on your host machine.

---

## 2. How to Run the Application

Bring up the entire runtime infrastructure, isolated database migrations, and application services automatically:

```bash
docker compose up --build
```


## 3. Core API Endpoints

The system exposes the following REST endpoints for merchant administration and checkout sequence execution:

| HTTP Method | Endpoint Path | Operational Action (Single-Line Summary) | `curl` Example |
| :--- | :--- | :--- | :--- |
| **`POST`** | `/api/v1/businesses` | Registers a new merchant profile and generates a unique webhook secret. | See Example 1 Below |
| **`POST`** | `/api/v1/customers` | Provisions a new client profile under a specific merchant's tenant scope. | See Example 2 Below |
| **`POST`** | `/api/v1/invoices` | Creates a new billing ledger record initialized in a `DRAFT` state. | See Example 3 Below |
| **`GET`** | `/api/v1/invoices/{id}` | Retrieves the current state and transactional history of a specific invoice. | See Example 4 Below |
| **`POST`** | `/api/v1/invoices/{id}/pay` | Acquires a database row-lock and passes payment details to the gateway. | See Example 5 & 6 Below |
| **`POST`** | `/api/v1/webhooks` | The ingress endpoint handling outbound event dispatches and retry payloads. | See Example 7 Below |


### Example 1: Register a Merchant Business
```bash
curl -X POST http://localhost:3000/api/v1/onboard \
  -H "Content-Type: application/json" \
  -d '{"name": "gg haded", "webhook_url": "https://api.gg.com"}'
```
### Example 2: Create a Customer Profile
```bash
curl -X POST http://localhost:3000/api/v1/customers \
  -H "Authorization: Bearer dodo_live_secret_key_here" \
  -H "Content-Type: application/json" \
  -d '{"name": "Afridi Shaik", "email": "afridi@example.com"}'
  ```
### Example 3: Generate a Draft Invoice
```bash
  curl -X POST http://localhost:3000/api/v1/invoices \
  -H "Authorization: Bearer dodo_live_secret_key_here" \
  -H "Content-Type: application/json" \
  -d '{"customer_id": "replace-with-customer-uuid", "total_amount_cents": 1550, "due_date": "2026-12-31T23:59:59Z"}'
```
### Example 4: Fetch Invoice Details
```bash
curl -X GET http://localhost:3000/api/v1/invoices/replace-with-invoice-uuid \
  -H "Authorization: Bearer dodo_live_secret_key_here"
  ```

### Example 5: Attempt Payment (Success Path)
``` bash
curl -X POST http://localhost:3000/api/v1/invoices/replace-with-invoice-uuid/pay \
  -H "Authorization: Bearer dodo_live_secret_key_here" \
  -H "Idempotency-Key: request_uniq_id_01" \
  -H "Content-Type: application/json" \
  -d '{"card_token": "tok_visa_success"}'
```
### Example 6: Attempt Payment (Failure Path - Card Declined)
```bash
curl -X POST http://localhost:3000/api/v1/invoices/replace-with-invoice-uuid/pay \
  -H "Authorization: Bearer dodo_live_secret_key_here" \
  -H "Idempotency-Key: request_uniq_id_02" \
  -H "Content-Type: application/json" \
  -d '{"card_token": "tok_card_declined"}'
```
### Example 7: Trigger Mock Webhook Event
```bash
curl -X POST http://localhost:3000/api/v1/webhooks \
  -H "X-Dodo-Signature: compute_hmac_signature_here" \
  -H "Content-Type: application/json" \
  -d '{"event": "invoice.paid", "invoice_id": "replace-with-invoice-uuid"}'
```

## 4. API Reference Documentation

The complete endpoint schema, strict request/response data shapes, and uniform RFC-7807 error formats are defined in our formal OpenAPI 3.0 specification file:

**[Explore the OpenAPI Specification (swagger.yaml)](./swagger.yaml)**

### How to Render Interactively
To visualize and test the API endpoints interactively using the graphical UI engine:
1. Copy the raw contents of the local `swagger.yaml` file.
2. Navigate to the official online **[Swagger Editor](https://editor.swagger.io/)**.
3. Paste the contents into the editor pane to instantly generate an interactive API testing sandbox on the right-hand panel.

---
## 5. Integrated Test Suite Matrix

The integration suite simulates intense traffic conditions, database locks, and external network drops to verify the system's structural durability.
### Local Test Execution Setup

To run these integration tests locally, ensure your database container is healthy and run the following command in your terminal shell:

```bash
DATABASE_URL=postgres://postgres:supersecretpassword@localhost:5432/dodo cargo test
```

* **`test_concurrent_payment_race_condition`**
  * **Description**: Fires multiple payment requests at the exact same millisecond to a single active invoice.
  * **Validation**: Asserts that PostgreSQL locks the row immediately, allowing only the first thread to execute while safely rejecting the rest to prevent double-charging.

* **`test_payment_idempotency_shield`**
  * **Description**: Retries duplicate payment payloads targeting the same invoice using identical idempotency keys.
  * **Validation**: Asserts that the request gate catches the duplicate key early, short-circuits execution, and replays the cached server response without hitting the bank gateway again.

* **`test_psp_timeout_graceful_handling`**
  * **Description**: Forces an artificial 30-second network latency delay (`tok_timeout`) on the external bank simulator connection.
  * **Validation**: Asserts that the client connection handles the 5-second HTTP deadline safely, returning an immediate response to the user while keeping the invoice record uncorrupted.