-- Accounting units: Gongbu quotes, authorizes and settles in micro-units
-- (10^-6 of the currency, `units_scale` 6), the same units Hubu charges.
-- The `*_minor` columns remain as whole-cent projections rounded up.
--
-- The columns are added by the repository before this backfill runs. The
-- backfill converts existing whole-cent rows exactly (×10^4), runs in one
-- transaction with an overflow guard and a verification query, and only
-- touches rows that have not been converted, so it is idempotent.
UPDATE executions
SET authorized_units = authorized_minor * 10000, units_scale = 6
WHERE authorized_units IS NULL;

UPDATE hubu_authorization_snapshots
SET amount_units = amount_minor * 10000, units_scale = 6
WHERE amount_units IS NULL;

UPDATE receipts
SET settlement_units = settlement_minor * 10000, units_scale = 6
WHERE settlement_units IS NULL;
