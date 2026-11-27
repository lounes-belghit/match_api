ALTER TABLE accounts ADD COLUMN IF NOT EXISTS is_admin BOOLEAN NOT NULL DEFAULT false;

CREATE TABLE IF NOT EXISTS payment_settings (
    enabled BOOLEAN PRIMARY KEY DEFAULT true CHECK (enabled),
    currency TEXT NOT NULL DEFAULT 'USDT',
    network TEXT NOT NULL DEFAULT '',
    wallet_address TEXT NOT NULL DEFAULT '',
    instructions TEXT NOT NULL DEFAULT '',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO payment_settings (enabled, currency, network, wallet_address, instructions)
VALUES (true, 'USDT', '', '', 'Send the exact amount on the configured network, then paste the transaction hash below. An administrator will review and apply the plan.')
ON CONFLICT (enabled) DO NOTHING;

CREATE TABLE IF NOT EXISTS payment_requests (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    plan TEXT NOT NULL REFERENCES plans(name),
    amount_micros BIGINT NOT NULL CHECK (amount_micros >= 0),
    currency TEXT NOT NULL DEFAULT 'USDT',
    network TEXT NOT NULL DEFAULT '',
    wallet_address TEXT NOT NULL DEFAULT '',
    tx_hash TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'approved', 'rejected')),
    review_note TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    reviewed_at TIMESTAMPTZ,
    reviewed_by UUID REFERENCES accounts(id)
);

CREATE INDEX IF NOT EXISTS payment_requests_status_created_at_idx
    ON payment_requests (status, created_at DESC);
CREATE INDEX IF NOT EXISTS payment_requests_account_created_at_idx
    ON payment_requests (account_id, created_at DESC);
