-- 1. Businesses Table (Auth storage)
CREATE TABLE businesses (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(255) NOT NULL,
    api_key_hash VARCHAR(64) NOT NULL UNIQUE, -- SHA-256 hex string
    webhook_secret VARCHAR(64) NOT NULL,
    webhook_url TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 2. Customers Table
CREATE TABLE customers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    business_id UUID NOT NULL REFERENCES businesses(id) ON DELETE CASCADE,
    name VARCHAR(255) NOT NULL,
    email VARCHAR(255) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(business_id, email) -- A customer belongs uniquely to a business by email
);

-- 3. Invoices Table
CREATE TABLE invoices (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    business_id UUID NOT NULL REFERENCES businesses(id) ON DELETE CASCADE,
    customer_id UUID NOT NULL REFERENCES customers(id) ON DELETE RESTRICT,
    state VARCHAR(50) NOT NULL, -- draft, open, paid, void, processing
    total_amount_cents BIGINT NOT NULL, -- Integer minor units
    due_date TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 4. Invoice Line Items (Stored as JSONB inside invoice or a side table. Let's do a side table for clarity)
CREATE TABLE invoice_items (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    invoice_id UUID NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
    description TEXT NOT NULL,
    quantity INT NOT NULL,
    unit_amount_cents BIGINT NOT NULL
);

-- 5. Payment Attempts Table
CREATE TABLE payment_attempts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    invoice_id UUID NOT NULL REFERENCES invoices(id) ON DELETE RESTRICT,
    status VARCHAR(50) NOT NULL, -- pending, success, failed
    card_token VARCHAR(255) NOT NULL,
    psp_reference UUID,
    failure_code VARCHAR(100),
    amount_cents BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 6. Idempotency Keys Table
CREATE TABLE idempotency_keys (
    idempotency_key VARCHAR(255) NOT NULL,
    business_id UUID NOT NULL REFERENCES businesses(id) ON DELETE CASCADE,
    request_path TEXT NOT NULL,
    request_hash VARCHAR(64) NOT NULL, -- To detect if body changed
    response_status INT,               -- NULL if still processing/crashed
    response_body TEXT,                -- NULL if still processing/crashed
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (idempotency_key, business_id)
);

-- Essential Performance Indexes
CREATE INDEX idx_invoices_business_state ON invoices(business_id, state);
CREATE INDEX idx_customers_business ON customers(business_id);
CREATE INDEX idx_payment_attempts_invoice ON payment_attempts(invoice_id);
