# AI Tools Usage Log

## 1. Tools Used & Specific Scopes

* **Gemini (This Chat Instance)**: Acted as my primary architectural sparring partner. I didn't just ask it to write code; I used it to format layouts, clean up dense text walls into readable Markdown, build out structural documentation like the OpenAPI `swagger.yaml` spec, and iteratively refine the physical database ER box diagrams.
* **Cursor / Copilot**: Used for high-speed inline editor autocomplete. It generated the standard boilerplate code, serialized structures, Rust struct tags, and formatted the sequential terminal `curl` execution paths.
* **ChatGPT**: Used as a quick reference tool during early engineering planning to weigh the trade-offs of Postgres advisory locks versus row-level pessimistic locking.

---

## 2. Independent Decisions (Where I pushed back or took control)

### Decision 1: Relational Composite Key Constraints vs. App-Level String Concatenation
* **AI Suggested**: Combine the tenant identifiers into a simple string payload (e.g., prefixing `merchantID_keyString`) to track multi-tenant uniqueness in a basic, flat database index.
* **My Choice**: I explicitly pushed back and enforced a strict database-level unique composite constraint: `PRIMARY KEY (idempotency_key, business_id)`.
* **Why**: Doing string manipulation in application code is a recipe for runtime bugs. Forcing the composite key straight into the PostgreSQL schema guarantees absolute multi-tenancy isolation at the data layer, making cross-tenant collisions physically impossible.

### Decision 2: Pessimistic Row Locking vs. Advisory Locks / OCC
* **AI Suggested**: Use **Postgres Advisory Locks** or **Optimistic Concurrency Control (OCC)** to keep code non-blocking and highly concurrent.
* **My Choice**: I chose a strict **Pessimistic Row Lock** (`SELECT ... FOR UPDATE`) inside an isolated transaction block.
* **Why**: Advisory locks detach the lock lifecycle from actual row state, which risks stuck or orphaned locks if a container crashes mid-flight. OCC forces expensive application-level retries under heavy traffic. Pessimistic locking handles concurrency safely at the database layer, ensuring only one thread can ever call out to the bank gateway at a time.

### Decision 3: Completely Asynchronous Out-of-Band Webhooks
* **AI Suggested**: Wrote standard handler boilerplate that fired the outbound webhook HTTP requests inline, right inside the synchronous API payment execution path.
* **My Choice**: I stripped it out and decoupled webhook delivery completely from the critical payment response loop using async green threads (`tokio::spawn`).
* **Why**: Inline webhooks make your API latency completely dependent on the health of your customer's server. Spawning the task out-of-band guarantees that our payment API returns a response to the user in milliseconds, fully shielding our platform from third-party network issues.

---

## 3. Technical Oversight & AI Corrections

### Implementing a Dedicated Onboarding Layer vs. Direct Merchant Spawning
* **AI Proposed**: The AI's initial setup route assumed merchants would just be blindly inserted into the `businesses` table via a basic `POST /businesses` endpoint, treating onboarding as a trivial CRUD operation.
* **My Choice**: I chose to separate the onboarding logic out and enforce an explicit, staged verification layer before a merchant is marked active or issued live credentials.
* **Why**: The AI completely ignored compliance and operational risk. In a real-world payments engine, you can't just let an unverified endpoint dump rows into your core multi-tenant configuration anchor. Designing a dedicated onboarding state path ensures we can handle KYC checks, verify webhook configuration targets, and validate egress setups *before* a business is allowed to touch live financial routing paths.