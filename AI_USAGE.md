# AI Tools Usage Log

## 1. Tools Used & Specific Scopes

* **Gemini (This Chat Instance)**: Acted as my primary architectural partner. Instead of just letting it blind-write code, I used it to clean up dense walls of text, structure the OpenAPI swagger.yaml specification, and iterate on structural layout definitions.
* **ChatGPT**: Used as a quick reference tool during early engineering planning to weigh the trade-offs of Postgres advisory locks versus row-level pessimistic locking.

Almost all of the documentation structuring and minor syntax debugging fixes were assisted by AI. And the Data models, flowchart was drawn by AI as well.
---

## 2. Independent Decisions (Where I pushed back or took control)

### Decision 1: Relational Composite Key Constraints vs. App-Level String Concatenation
* **AI Suggested**: Combine the tenant identifiers into a simple string payload (e.g., prefixing `merchantID_keyString`) to track multi-tenant uniqueness in a basic, flat database index.
* **My Choice**: I explicitly pushed back and enforced a strict database-level unique composite constraint: `PRIMARY KEY (idempotency_key, business_id)`.
* **Why**: Doing string manipulation in application code is a recipe for runtime bugs. Forcing the composite key straight into the PostgreSQL schema guarantees absolute multi-tenancy isolation at the data layer, making cross-tenant collisions physically impossible.


### Decision 4: Implementing a Timeout Guard for "In-Flight" Requests
* **AI Suggested**: The AI generated an asynchronous execution loop where duplicate requests arriving during a slow gateway transaction were served an indefinite `202 Accepted (PROCESSING)` state payload.
* **My Choice**: I explicitly updated the architecture to handle timeout failures gracefully, ensuring that if a background worker times out or panics, the invoice rolls back to an `OPEN` state instead of getting stuck in `PROCESSING` forever.
* **Why**: The AI's design assumed a perfect world where background workers never die. In reality, if a thread panics or the gateway hangs indefinitely, the invoice gets orphaned in a locked `PROCESSING` state limbo, blocking the customer from ever retrying. I added safety boundaries so that failed or timed-out background executions explicitly release the state lock, allowing users to safely re-attempt payment.

---

## 3. Technical Oversight & AI Corrections

### Implementing a Dedicated Onboarding Layer vs. Direct Merchant Spawning
* **AI Proposed**: The AI's initial setup route assumed merchants would just be blindly inserted into the `businesses` table via a basic `POST /businesses` endpoint, treating onboarding as a trivial CRUD operation.
* **My Choice**: I chose to separate the onboarding logic out and enforce an explicit, staged verification layer before a merchant is marked active or issued live credentials.
* **Why**: The AI completely ignored compliance and operational risk. In a real-world payments engine, you can't just let an unverified endpoint dump rows into your core multi-tenant configuration anchor. Designing a dedicated onboarding state path ensures we can handle KYC checks, verify webhook configuration targets, and validate egress setups *before* a business is allowed to touch live financial routing paths.