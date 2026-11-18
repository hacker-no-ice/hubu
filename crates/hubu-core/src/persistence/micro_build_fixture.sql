-- Frozen dual-written schema from the µ build, before HUB-284. Test fixture only.
CREATE TABLE budget_versions_micro_build (
    id TEXT PRIMARY KEY,
    budget_id TEXT NOT NULL,
    revision INTEGER NOT NULL,
    predecessor_version_id TEXT,
    amount_limit_cents INTEGER NOT NULL,
    effective_at TEXT NOT NULL,
    actor TEXT NOT NULL,
    source TEXT NOT NULL,
    reason TEXT,
    request_fingerprint TEXT NOT NULL,
    created_at TEXT NOT NULL,
    amount_limit INTEGER,
    amount_scale INTEGER,
    UNIQUE(budget_id, id),
    UNIQUE(budget_id, revision),
    UNIQUE(budget_id, predecessor_version_id),
    CHECK(revision >= 1),
    CHECK(amount_limit_cents > 0),
    CHECK(length(actor) > 0),
    CHECK(length(source) > 0),
    CHECK(length(request_fingerprint) > 0),
    FOREIGN KEY(budget_id) REFERENCES budgets(id),
    FOREIGN KEY(budget_id, predecessor_version_id)
                    REFERENCES budget_versions(budget_id, id)
);

CREATE TABLE budget_balances_micro_build (
    budget_id TEXT PRIMARY KEY,
    consumed_amount_cents INTEGER NOT NULL,
    frozen_amount_cents INTEGER NOT NULL,
    remaining_amount_cents INTEGER NOT NULL,
    updated_at TEXT NOT NULL,
    consumed_amount INTEGER,
    frozen_amount INTEGER,
    remaining_amount INTEGER,
    amount_scale INTEGER,
    CHECK(consumed_amount_cents >= 0),
    CHECK(frozen_amount_cents >= 0),
    FOREIGN KEY(budget_id) REFERENCES budgets(id)
);

CREATE TABLE budget_holds_micro_build (
    id TEXT PRIMARY KEY,
    budget_id TEXT NOT NULL,
    budget_version_id TEXT NOT NULL,
    spend_decision_id TEXT NOT NULL,
    amount_cents INTEGER NOT NULL,
    currency TEXT NOT NULL,
    status TEXT NOT NULL,
    executor_claim_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    amount INTEGER,
    amount_scale INTEGER,
    spend_asset TEXT,
    spend_exact_amount TEXT,
    spend_exact_scale INTEGER,
    conversion_json TEXT,
    CHECK(amount_cents > 0),
    FOREIGN KEY(budget_id) REFERENCES budgets(id),
    FOREIGN KEY(budget_id, budget_version_id)
                    REFERENCES budget_versions(budget_id, id),
    FOREIGN KEY(spend_decision_id) REFERENCES spend_decisions(id)
);

CREATE TABLE spending_targets_micro_build (
    id TEXT PRIMARY KEY,
    owner_user_id TEXT NOT NULL,
    target_amount_cents INTEGER NOT NULL,
    currency TEXT NOT NULL,
    starting_at TEXT NOT NULL,
    ending_before TEXT,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    target_amount INTEGER,
    amount_scale INTEGER
);

CREATE TABLE spend_executor_settlement_receipts_micro_build (
    claim_id TEXT PRIMARY KEY,
    settlement_id TEXT NOT NULL UNIQUE,
    authorized_max_cents INTEGER NOT NULL,
    actual_vendor_cost_cents INTEGER NOT NULL,
    actual_vendor_cost_amount INTEGER NOT NULL,
    actual_vendor_cost_scale INTEGER NOT NULL,
    actual_vendor_cost_currency TEXT NOT NULL,
    budget_charge_cents INTEGER NOT NULL,
    released_amount_cents INTEGER NOT NULL,
    overrun_amount_cents INTEGER NOT NULL,
    currency TEXT NOT NULL,
    provider_request_id TEXT NOT NULL,
    price_model_snapshot_json TEXT NOT NULL,
    artifact_reference TEXT NOT NULL,
    created_at TEXT NOT NULL,
    authorized_max INTEGER,
    budget_charge INTEGER,
    released_amount INTEGER,
    overrun_amount INTEGER,
    amount_scale INTEGER,
    cost_basis TEXT NOT NULL DEFAULT 'legacy_unspecified',
    CHECK(authorized_max_cents > 0),
    CHECK(actual_vendor_cost_cents >= 0),
    CHECK(actual_vendor_cost_cents <= authorized_max_cents),
    CHECK(released_amount_cents = authorized_max_cents - actual_vendor_cost_cents),
    CHECK(actual_vendor_cost_amount >= 0),
    CHECK(actual_vendor_cost_scale BETWEEN 0 AND 18),
    CHECK(budget_charge_cents >= 0),
    CHECK(overrun_amount_cents >= 0),
    FOREIGN KEY(claim_id) REFERENCES spend_executor_claims(id)
);
