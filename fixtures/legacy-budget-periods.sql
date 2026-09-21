-- Compatibility data emitted by the pre-removal BudgetManager at
-- 2c3a2295d76342b39475a64860322b7908374ab0 (two daily periods).
-- This is historical persisted state, not a supported creation example.
INSERT INTO "budgets" VALUES('f571c264-fbee-4567-872a-3ad90361b29d','agent','ba02a82b-bec1-41f7-8440-6e306bc18d8d','usd','2030-01-01T00:00:00+00:00','2030-01-02T00:00:00+00:00','active','2026-09-21T19:18:46.306773+00:00','2026-09-21T19:18:46.306773+00:00');
INSERT INTO "budgets" VALUES('8209b42d-3a3a-4211-ae20-474416c67d7b','agent','ba02a82b-bec1-41f7-8440-6e306bc18d8d','usd','2030-01-02T00:00:00+00:00','2030-01-03T00:00:00+00:00','active','2026-09-21T19:18:46.306812+00:00','2026-09-21T19:18:46.306812+00:00');
INSERT INTO "budget_versions" VALUES('8d6032cc-ffa7-4665-a1cc-6958a8b8d059','f571c264-fbee-4567-872a-3ad90361b29d',1,NULL,10000,'2026-09-21T19:18:46.306773+00:00','legacy-human','hubu-api:create-budget-series',NULL,'sha256:eb1d2a9f2de0e2b68e7f891a3a3d46eea2af25916d172461328134c286242634','2026-09-21T19:18:46.306773+00:00');
INSERT INTO "budget_versions" VALUES('ec564712-fd9c-4ff0-b5d2-1d291243fda5','8209b42d-3a3a-4211-ae20-474416c67d7b',1,NULL,10000,'2026-09-21T19:18:46.306812+00:00','legacy-human','hubu-api:create-budget-series',NULL,'sha256:85f094fcb3e1715feeb81c960be0445dfa11e6ed4afbcb4e8c854dfc57487602','2026-09-21T19:18:46.306812+00:00');
INSERT INTO "budget_current_versions" VALUES('f571c264-fbee-4567-872a-3ad90361b29d','8d6032cc-ffa7-4665-a1cc-6958a8b8d059');
INSERT INTO "budget_current_versions" VALUES('8209b42d-3a3a-4211-ae20-474416c67d7b','ec564712-fd9c-4ff0-b5d2-1d291243fda5');
INSERT INTO "budget_balances" VALUES('f571c264-fbee-4567-872a-3ad90361b29d',0,0,10000,'2026-09-21T19:18:46.307344+00:00');
INSERT INTO "budget_balances" VALUES('8209b42d-3a3a-4211-ae20-474416c67d7b',0,0,10000,'2026-09-21T19:18:46.307816+00:00');
