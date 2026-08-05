-- Money is integer minor units; timestamps are UTC RFC 3339. Every JSON value
-- has a positive format version. Lifecycle/outcome/provider fields are nullable.
CREATE TABLE IF NOT EXISTS executions(
 execution_id TEXT PRIMARY KEY, account_id TEXT NOT NULL CHECK(trim(account_id)<>''), operation_key TEXT NOT NULL CHECK(trim(operation_key)<>''),
 hubu_authorization_id TEXT NOT NULL, hubu_claim_id TEXT, hubu_token_reference TEXT NOT NULL CHECK(length(hubu_token_reference) BETWEEN 1 AND 255),
 authorized_minor INTEGER NOT NULL CHECK(authorized_minor>=0), authorization_currency TEXT NOT NULL CHECK(length(authorization_currency)=3),
 normalized_input_json TEXT NOT NULL CHECK(json_valid(normalized_input_json)), input_hash TEXT NOT NULL, input_schema_version INTEGER NOT NULL CHECK(input_schema_version>0),
 target TEXT NOT NULL, config_version TEXT NOT NULL, pricing_snapshot_json TEXT NOT NULL CHECK(json_valid(pricing_snapshot_json)), pricing_schema_version INTEGER NOT NULL CHECK(pricing_schema_version>0),
 status TEXT NOT NULL CHECK(status IN ('pending','running','succeeded','failed','canceled','reconciliation_required')), outcome TEXT, failure_code TEXT, failure_message_redacted TEXT,
 created_at TEXT NOT NULL, updated_at TEXT NOT NULL, started_at TEXT, completed_at TEXT, version INTEGER NOT NULL DEFAULT 0 CHECK(version>=0), UNIQUE(account_id,operation_key));
CREATE INDEX IF NOT EXISTS executions_status_created ON executions(status,created_at);
CREATE INDEX IF NOT EXISTS executions_claim ON executions(hubu_claim_id) WHERE hubu_claim_id IS NOT NULL;
CREATE TABLE IF NOT EXISTS provider_attempts(
 provider_attempt_id TEXT PRIMARY KEY, execution_id TEXT NOT NULL REFERENCES executions ON DELETE CASCADE, provider TEXT NOT NULL,
 provider_request_id TEXT, provider_operation_id TEXT, outcome TEXT NOT NULL CHECK(outcome IN ('started','succeeded','failed','ambiguous','canceled')),
 usage_json TEXT CHECK(usage_json IS NULL OR json_valid(usage_json)), usage_schema_version INTEGER CHECK(usage_schema_version IS NULL OR usage_schema_version>0),
 provider_amount_minor INTEGER CHECK(provider_amount_minor IS NULL OR provider_amount_minor>=0), provider_currency TEXT, failure_code TEXT, failure_message_redacted TEXT,
 started_at TEXT NOT NULL, completed_at TEXT, CHECK((usage_json IS NULL)=(usage_schema_version IS NULL)), CHECK((provider_amount_minor IS NULL)=(provider_currency IS NULL)));
CREATE INDEX IF NOT EXISTS attempts_execution_started ON provider_attempts(execution_id,started_at);
CREATE UNIQUE INDEX IF NOT EXISTS attempts_provider_request ON provider_attempts(provider,provider_request_id) WHERE provider_request_id IS NOT NULL;
CREATE TABLE IF NOT EXISTS artifacts(
 artifact_id TEXT PRIMARY KEY, execution_id TEXT NOT NULL REFERENCES executions ON DELETE CASCADE, provider_attempt_id TEXT REFERENCES provider_attempts ON DELETE SET NULL,
 kind TEXT NOT NULL, storage_backend TEXT NOT NULL, media_type TEXT NOT NULL, storage_key TEXT NOT NULL UNIQUE, size_bytes INTEGER NOT NULL CHECK(size_bytes>=0), sha256 TEXT NOT NULL,
 metadata_json TEXT NOT NULL CHECK(json_valid(metadata_json)), metadata_schema_version INTEGER NOT NULL CHECK(metadata_schema_version>0), created_at TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS artifacts_execution_created ON artifacts(execution_id,created_at);
CREATE TABLE IF NOT EXISTS receipts(
 receipt_id TEXT PRIMARY KEY, execution_id TEXT NOT NULL UNIQUE REFERENCES executions ON DELETE RESTRICT,
 provider_attempt_id TEXT NOT NULL UNIQUE REFERENCES provider_attempts ON DELETE RESTRICT, settlement_minor INTEGER NOT NULL CHECK(settlement_minor>=0),
 currency TEXT NOT NULL CHECK(length(currency)=3), pricing_catalog_version TEXT NOT NULL, created_at TEXT NOT NULL, settled_at TEXT, hubu_settlement_id TEXT UNIQUE);
CREATE INDEX IF NOT EXISTS receipts_settlement ON receipts(hubu_settlement_id) WHERE hubu_settlement_id IS NOT NULL;
