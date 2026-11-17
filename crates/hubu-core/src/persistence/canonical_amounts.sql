-- HUB-284: µ columns are the only accounting representation.
CREATE TABLE IF NOT EXISTS budget_versions (
    id TEXT PRIMARY KEY,
    budget_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    predecessor_version_id TEXT,
    effective_at TEXT NOT NULL,
    actor TEXT NOT NULL,
    source TEXT NOT NULL,
    reason TEXT,
    request_fingerprint TEXT NOT NULL,
    created_at TEXT NOT NULL,
    amount_limit INTEGER NOT NULL CHECK(typeof(amount_limit) = 'integer' AND amount_limit > 0),
    amount_scale INTEGER NOT NULL CHECK(amount_scale = 6),
    UNIQUE(budget_id, id),
    UNIQUE(budget_id, revision),
    UNIQUE(budget_id, predecessor_version_id),
    CHECK(revision >= 1),
    CHECK(length(actor) > 0),
    CHECK(length(source) > 0),
    CHECK(length(request_fingerprint) > 0),
    FOREIGN KEY(budget_id) REFERENCES budgets(id),
    FOREIGN KEY(budget_id, predecessor_version_id)
    REFERENCES budget_versions(budget_id, id)
);

CREATE TABLE IF NOT EXISTS budget_balances (
    budget_id TEXT PRIMARY KEY,
    updated_at TEXT NOT NULL,
    consumed_amount INTEGER NOT NULL CHECK(typeof(consumed_amount) = 'integer' AND consumed_amount >= 0),
    frozen_amount INTEGER NOT NULL CHECK(typeof(frozen_amount) = 'integer' AND frozen_amount >= 0),
    remaining_amount INTEGER NOT NULL CHECK(typeof(remaining_amount) = 'integer' ),
    amount_scale INTEGER NOT NULL CHECK(amount_scale = 6),
    FOREIGN KEY(budget_id) REFERENCES budgets(id)
);

CREATE TABLE IF NOT EXISTS budget_holds (
    id TEXT PRIMARY KEY,
    budget_id TEXT NOT NULL,
    budget_version_id TEXT NOT NULL,
    spend_decision_id TEXT NOT NULL,
    currency TEXT NOT NULL,
    status TEXT NOT NULL,
    executor_claim_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    amount INTEGER NOT NULL CHECK(typeof(amount) = 'integer' AND amount > 0),
    spend_asset TEXT NOT NULL,
    spend_exact_amount TEXT NOT NULL,
    spend_exact_scale INTEGER NOT NULL CHECK(spend_exact_scale BETWEEN 0 AND 18),
    conversion_json TEXT,
    amount_scale INTEGER NOT NULL CHECK(amount_scale = 6),
    FOREIGN KEY(budget_id) REFERENCES budgets(id),
    FOREIGN KEY(budget_id, budget_version_id)
    REFERENCES budget_versions(budget_id, id),
    FOREIGN KEY(spend_decision_id) REFERENCES spend_decisions(id)
);

CREATE TABLE IF NOT EXISTS spending_targets (
    id TEXT PRIMARY KEY,
    owner_user_id TEXT NOT NULL,
    currency TEXT NOT NULL,
    starting_at TEXT NOT NULL,
    ending_before TEXT,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    target_amount INTEGER NOT NULL CHECK(typeof(target_amount) = 'integer' AND target_amount > 0),
    amount_scale INTEGER NOT NULL CHECK(amount_scale = 6)
);

CREATE TABLE IF NOT EXISTS spend_executor_settlement_receipts (
    claim_id TEXT PRIMARY KEY,
    settlement_id TEXT NOT NULL UNIQUE,
    actual_vendor_cost_amount INTEGER NOT NULL,
    actual_vendor_cost_scale INTEGER NOT NULL,
    actual_vendor_cost_currency TEXT NOT NULL,
    currency TEXT NOT NULL,
    provider_request_id TEXT NOT NULL,
    price_model_snapshot_json TEXT NOT NULL,
    artifact_reference TEXT NOT NULL,
    created_at TEXT NOT NULL,
    authorized_max INTEGER NOT NULL CHECK(typeof(authorized_max) = 'integer' AND authorized_max > 0),
    budget_charge INTEGER NOT NULL CHECK(typeof(budget_charge) = 'integer' AND budget_charge >= 0),
    released_amount INTEGER NOT NULL CHECK(typeof(released_amount) = 'integer' AND released_amount >= 0),
    overrun_amount INTEGER NOT NULL CHECK(typeof(overrun_amount) = 'integer' AND overrun_amount >= 0),
    cost_basis TEXT NOT NULL DEFAULT 'legacy_unspecified',
    amount_scale INTEGER NOT NULL CHECK(amount_scale = 6),
    CHECK(actual_vendor_cost_amount >= 0),
    CHECK(actual_vendor_cost_scale BETWEEN 0 AND 18),
    FOREIGN KEY(claim_id) REFERENCES spend_executor_claims(id)
);
