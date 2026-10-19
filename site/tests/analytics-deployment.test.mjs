import assert from "node:assert/strict";
import test from "node:test";
import { configureAnalytics } from "../scripts/configure-analytics.mjs";

const config = { vars: { ANALYTICS_ORIGIN: "https://hubustack.dev" }, d1_databases: [{ binding: "ANALYTICS_DB" }, { binding: "OTHER", database_id: "preserved" }], triggers: { crons: ["17 3 * * *"] } };
const settings = { HUBUSTACK_ANALYTICS_DB_ID: "12345678-1234-1234-1234-123456789abc", HUBUSTACK_ANALYTICS_ACCESS_TEAM_DOMAIN: "example.cloudflareaccess.com", HUBUSTACK_ANALYTICS_ACCESS_AUD: "test-audience" };

test("unconfigured deployments keep public pages running without provisioning production storage", () => {
  const prepared = configureAnalytics(config, {}, "/migrations");
  assert.equal(prepared.vars.ANALYTICS_ENABLED, "false");
  assert.deepEqual(prepared.d1_databases, [{ binding: "OTHER", database_id: "preserved" }]);
  assert.deepEqual(prepared.triggers.crons, []);
  assert.equal(config.d1_databases.length, 2);
});

test("configured deployments pin one existing database and retain maintenance", () => {
  const prepared = configureAnalytics(config, settings, "/migrations");
  assert.equal(prepared.vars.ANALYTICS_ENABLED, "true");
  assert.equal(prepared.vars.ANALYTICS_ACCESS_TEAM_DOMAIN, settings.HUBUSTACK_ANALYTICS_ACCESS_TEAM_DOMAIN);
  assert.deepEqual(prepared.d1_databases.at(-1), { binding: "ANALYTICS_DB", database_name: "hubustack-analytics", database_id: settings.HUBUSTACK_ANALYTICS_DB_ID, migrations_dir: "/migrations" });
  assert.deepEqual(configureAnalytics(prepared, settings, "/migrations"), prepared);
  const disabled = configureAnalytics(prepared, {}, "/migrations");
  assert.deepEqual(configureAnalytics(disabled, settings, "/migrations").triggers.crons, ["17 3 * * *"]);
});

test("partial and unsafe configuration fails before deployment", () => {
  for (const value of [{ HUBUSTACK_ANALYTICS_DB_ID: settings.HUBUSTACK_ANALYTICS_DB_ID }, { ...settings, HUBUSTACK_ANALYTICS_ACCESS_TEAM_DOMAIN: "https://wrong.test" }, { ...settings, HUBUSTACK_ANALYTICS_DB_ID: "invalid" }, { ...settings, HUBUSTACK_ANALYTICS_ACCESS_AUD: " " }]) {
    assert.throws(() => configureAnalytics(config, value, "/migrations"), /all three analytics deployment variables/);
  }
});
