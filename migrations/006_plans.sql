CREATE TABLE IF NOT EXISTS plans (
    name TEXT PRIMARY KEY,
    monthly_quota BIGINT NOT NULL CHECK (monthly_quota > 0),
    rate_per_min INTEGER NOT NULL CHECK (rate_per_min > 0),
    description TEXT NOT NULL DEFAULT ''
);

INSERT INTO plans (name, monthly_quota, rate_per_min, description) VALUES
    ('free', 10000, 60, 'Personal projects and evaluations'),
    ('pro', 1000000, 300, 'Production applications'),
    ('business', 10000000, 1000, 'High-volume commercial use')
ON CONFLICT (name) DO UPDATE SET
    monthly_quota = EXCLUDED.monthly_quota,
    rate_per_min = EXCLUDED.rate_per_min,
    description = EXCLUDED.description;

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'accounts_plan_fkey') THEN
        ALTER TABLE accounts ADD CONSTRAINT accounts_plan_fkey
            FOREIGN KEY (plan) REFERENCES plans(name);
    END IF;
END $$;
