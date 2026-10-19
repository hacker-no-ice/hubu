import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { DatabaseSync } from "node:sqlite";
import test from "node:test";
import { buildReport, handleCollect, pruneAnalytics } from "../worker/analytics.ts";

const migration = await readFile(new URL("../migrations/0001_analytics.sql", import.meta.url), "utf8");
const ORIGIN = "https://hubustack.dev";
const DAY = 86_400_000;
const NOW = Date.parse("2026-10-19T12:00:00Z");

// Exercise the production migration and SQL with real SQLite. This adapter only
// implements the D1 calls in the module; batch retains D1 transaction semantics.
class TestDatabase {
  constructor(t) {
    this.sqlite = new DatabaseSync(":memory:");
    this.sqlite.exec("PRAGMA foreign_keys = ON");
    this.sqlite.exec(migration);
    t.after(() => this.sqlite.close());
  }

  prepare(sql) {
    const statement = this.sqlite.prepare(sql);
    const execute = (values = []) => {
      if (/^\s*(SELECT|WITH)\b/i.test(sql)) return { results: statement.all(...values).map((row) => ({ ...row })), meta: { changes: 0 } };
      const result = statement.run(...values);
      return { results: [], meta: { changes: Number(result.changes) } };
    };
    const bound = (values = []) => ({
      execute: () => execute(values),
      bind: (...nextValues) => bound(nextValues),
      run: async () => execute(values),
    });
    return bound();
  }

  async batch(statements) {
    this.sqlite.exec("BEGIN");
    try {
      const results = statements.map((statement) => statement.execute());
      this.sqlite.exec("COMMIT");
      return results;
    } catch (error) {
      this.sqlite.exec("ROLLBACK");
      throw error;
    }
  }

  rows(table) {
    assert.ok(["analytics_events", "analytics_visitors", "analytics_exclusions"].includes(table));
    return this.sqlite.prepare(`SELECT * FROM ${table}`).all();
  }
}

function setup(t) {
  t.mock.method(Date, "now", () => NOW);
  const db = new TestDatabase(t);
  return { db, env: { ANALYTICS_DB: db, ANALYTICS_ORIGIN: ORIGIN, ANALYTICS_ENABLED: "true" } };
}

function beacon(overrides = {}) {
  return { eventId: crypto.randomUUID(), sessionId: crypto.randomUUID(), path: "/docs/overview", referrer: "", automated: false, ...overrides };
}

function request(body = beacon(), options = {}) {
  const { headers, cf, ...rest } = options;
  const req = new Request(`${ORIGIN}/_telemetry/collect`, {
    method: "POST",
    body: typeof body === "string" ? body : JSON.stringify(body),
    ...rest,
    headers: { origin: ORIGIN, "content-type": "application/json", "sec-fetch-site": "same-origin", "user-agent": "Mozilla/5.0", ...headers },
  });
  if (cf) Object.defineProperty(req, "cf", { value: cf });
  return req;
}

function responseCookie(response) {
  return response.headers.get("set-cookie")?.split(";")[0];
}

test("collection hashes browser identity, strips URL details, and deduplicates retries", async (t) => {
  const { db, env } = setup(t);
  const initial = beacon({ path: "/docs/overview/?sensitive=token#secret", referrer: "https://t.co/private-path?token=secret" });
  const first = await handleCollect(request(initial, { cf: { country: "US" } }), env);
  assert.equal(first.status, 204);
  assert.match(first.headers.get("set-cookie"), /^__Host-hubu_visitor=[a-f0-9-]+; Path=\/; Secure; HttpOnly; SameSite=Lax; Max-Age=31536000$/);
  const cookie = responseCookie(first);
  const visitor = cookie.split("=")[1];
  const [row] = db.rows("analytics_events");
  assert.equal(row.path, "/docs/overview");
  assert.equal(row.referrer, "t.co");
  assert.equal(row.country, "US");
  assert.match(row.visitor_hash, /^[a-f0-9]{64}$/);
  assert.notEqual(row.visitor_hash, visitor);
  assert.equal(JSON.stringify([...db.rows("analytics_events"), ...db.rows("analytics_visitors")]).includes(visitor), false);
  assert.equal((await handleCollect(request(initial, { headers: { cookie } }), env)).status, 204);
  // A lost first response must not turn the retry into a second visitor, even
  // when the browser never received the cookie in that response.
  const replay = await handleCollect(request(initial), env);
  assert.equal(replay.status, 204);
  assert.equal(responseCookie(replay), undefined);
  assert.equal(db.rows("analytics_events").length, 1);
  assert.equal(db.rows("analytics_visitors").length, 1);
  const second = await handleCollect(request(beacon({ sessionId: initial.sessionId, referrer: `${ORIGIN}/docs?q=private` }), { headers: { cookie } }), env);
  assert.equal(responseCookie(second), undefined);
  assert.equal(db.rows("analytics_events").length, 2);
  assert.equal(db.rows("analytics_events")[1].referrer, "");
  const report = await buildReport(env, 7);
  assert.deepEqual(report.summary, { uniqueVisitors: 1, visits: 1, pageViews: 2, newVisitors: 1, returningVisitors: 0, excludedEvents: 0 });
  assert.deepEqual(report.referrers, [{ referrer: "t.co", visits: 1, visitors: 1 }], "arrival order breaks same-millisecond session ties");
});

test("malformed visitor cookies are replaced with a server-generated cookie", async (t) => {
  const { db, env } = setup(t);
  const result = await handleCollect(request(beacon(), { headers: { cookie: "__Host-hubu_visitor=attacker-controlled" } }), env);
  assert.equal(result.status, 204);
  assert.match(responseCookie(result), /^__Host-hubu_visitor=[a-f0-9-]{36}$/);
  assert.equal(db.rows("analytics_visitors").length, 1);
});

test("privacy preferences and disabled deployments write nothing and issue no identity", async (t) => {
  const { db, env } = setup(t);
  let limiterCalls = 0;
  env.ANALYTICS_RATE_LIMITER = { limit: async () => { limiterCalls += 1; return { success: true }; } };
  for (const headers of [{ dnt: "1" }, { "sec-gpc": "1" }, { cookie: "other=x; hubu_analytics_exclude=1" }]) {
    const result = await handleCollect(request(beacon(), { headers }), env);
    assert.equal(result.status, 204);
    assert.equal(responseCookie(result), undefined);
  }
  for (const disabled of [{ ...env, ANALYTICS_ENABLED: "false" }, { ...env, ANALYTICS_ENABLED: undefined }, { ...env, ANALYTICS_DB: undefined }]) {
    assert.equal((await handleCollect(request(), disabled)).status, 204);
    const report = await buildReport(disabled, 7);
    assert.equal(report.enabled, false);
    assert.equal(report.summary.uniqueVisitors, 0);
    assert.equal(report.daily.length, 7);
  }
  assert.equal(limiterCalls, 0);
  assert.equal(db.rows("analytics_events").length, 0);
  assert.equal(db.rows("analytics_visitors").length, 0);
  assert.equal(db.rows("analytics_exclusions").length, 0);
});

test("only bounded, valid same-origin browser page beacons are accepted", async (t) => {
  const { db, env } = setup(t);
  const badRequests = [
    [new Request(`${ORIGIN}/_telemetry/collect`), 405],
    [request(beacon(), { headers: { origin: "https://evil.example" } }), 403],
    [request(beacon(), { headers: { "sec-fetch-site": "cross-site" } }), 403],
    [request(beacon(), { headers: { "content-type": "text/plain" } }), 415],
    [request("[1]"), 400],
    [request("{"), 400],
    [request(beacon({ eventId: "bad" })), 400],
    [request(beacon({ sessionId: "bad" })), 400],
    [request(beacon({ automated: "false" })), 400],
    [request(beacon({ extra: "private" })), 400],
    [request(beacon({ path: "/".repeat(513) })), 400],
    [request(beacon({ path: "//evil.example/path" })), 400],
    [request(beacon({ path: "/_analytics" })), 400],
    [request(beacon({ path: "/_telemetry/collect" })), 400],
    [request(beacon({ path: "/%5fanalytics/report" })), 400],
    [request(beacon({ path: "/_next/static/chunk.js" })), 400],
    [request(beacon({ path: "/_vinext/image" })), 400],
    [request(beacon({ path: "/cdn-cgi/trace" })), 400],
    [request(beacon({ path: "/brand/logo.png" })), 400],
    [request(beacon({ path: "/.git/config" })), 400],
    [request(beacon({ path: "/bad%00path" })), 400],
    [request(" ".repeat(2049)), 413],
    [request(beacon(), { headers: { "content-length": "2049" } }), 413],
  ];
  for (const [req, expected] of badRequests) assert.equal((await handleCollect(req, env)).status, expected);
  // The configured canonical origin must also match the incoming request URL.
  const alternate = new Request("https://preview.example/_telemetry/collect", request());
  assert.equal((await handleCollect(alternate, env)).status, 403);
  assert.equal((await handleCollect(request(), { ...env, ANALYTICS_ORIGIN: `${ORIGIN}/path` })).status, 403);
  assert.equal(db.rows("analytics_events").length, 0);
  assert.equal(db.rows("analytics_visitors").length, 0);
});

test("HTTP collection is allowed only for explicitly configured localhost development", async (t) => {
  const { db, env } = setup(t);
  const localOrigin = "http://localhost:5173";
  const local = new Request(`${localOrigin}/_telemetry/collect`, request());
  local.headers.set("origin", localOrigin);
  const result = await handleCollect(local, { ...env, ANALYTICS_ORIGIN: localOrigin });
  assert.equal(result.status, 204);
  assert.ok(responseCookie(result));
  assert.equal(db.rows("analytics_events").length, 1);
  for (const origin of ["http://hubustack.dev", "http://localhost.evil.example", "http://127.0.0.1:5173", `${localOrigin}/path`, `${localOrigin}/?query=1`, "https://user:password@hubustack.dev"]) {
    const url = new URL(origin);
    const invalid = new Request(`${url.origin}/_telemetry/collect`, request());
    invalid.headers.set("origin", url.origin);
    assert.equal((await handleCollect(invalid, { ...env, ANALYTICS_ORIGIN: origin })).status, 403, origin);
  }
  assert.equal(db.rows("analytics_events").length, 1);
});

test("bot signals produce anonymous daily counts without issuing visitor cookies", async (t) => {
  const { db, env } = setup(t);
  for (const [body, options] of [
    [beacon({ automated: true }), {}],
    [beacon(), { cf: { botManagement: { verifiedBot: true, score: 99 } } }],
    [beacon(), { cf: { botManagement: { score: 12 } } }],
    [beacon(), { headers: { "user-agent": "Googlebot/2.1" } }],
  ]) {
    const result = await handleCollect(request(body, options), env);
    assert.equal(result.status, 204);
    assert.equal(responseCookie(result), undefined);
  }
  assert.equal(db.rows("analytics_events").length, 0);
  assert.equal(db.rows("analytics_visitors").length, 0);
  const report = await buildReport(env, 1);
  assert.equal(report.summary.excludedEvents, 4);
  assert.equal(report.daily[0].excludedEvents, 4);
  assert.deepEqual(report.exclusions.map(({ reason }) => reason).sort(), ["browser_automation", "crawler_user_agent", "low_bot_score", "verified_bot"]);
  assert.deepEqual(Object.keys(db.rows("analytics_exclusions")[0]).sort(), ["count", "day", "reason"]);
});

test("rate limits use a hashed in-memory key and short-circuit database writes", async (t) => {
  const { db, env } = setup(t);
  const keys = [];
  env.ANALYTICS_RATE_LIMITER = { limit: async ({ key }) => { keys.push(key); return { success: false }; } };
  const result = await handleCollect(request(beacon(), { headers: { "cf-connecting-ip": "192.0.2.1" } }), env);
  assert.equal(result.status, 429);
  assert.equal(result.headers.get("retry-after"), "60");
  assert.equal(responseCookie(result), undefined);
  assert.match(keys[0], /^[a-f0-9]{64}$/);
  assert.notEqual(keys[0], "192.0.2.1");
  await handleCollect(request(), env);
  await handleCollect(request(), env);
  assert.equal(keys[1], keys[2], "missing IPs must share a key instead of bypassing limits");
  assert.equal(db.rows("analytics_events").length, 0);
  assert.equal(db.rows("analytics_exclusions").length, 0);
});

function seedVisitor(db, hash, first, last = first) {
  db.sqlite.prepare("INSERT INTO analytics_visitors VALUES (?, ?, ?)").run(hash, first, last);
}

function seedEvent(db, { hash, time, session = "shared-session", path = "/", referrer = "", country = "US" }) {
  db.sqlite.prepare("INSERT INTO analytics_events VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
    .run(crypto.randomUUID(), hash, session, time, new Date(time).toISOString().slice(0, 10), path, referrer, country);
}

test("period reports count exact browser distincts, scope sessions by browser, and use landing referrers", async (t) => {
  const { db, env } = setup(t);
  seedVisitor(db, "returning", NOW - 10 * DAY, NOW);
  seedVisitor(db, "new-one", NOW - DAY, NOW);
  seedVisitor(db, "new-two", NOW, NOW);
  seedEvent(db, { hash: "returning", time: NOW - DAY, referrer: "t.co" });
  seedEvent(db, { hash: "returning", time: NOW, path: "/docs" });
  seedEvent(db, { hash: "new-one", time: NOW - DAY, path: "/docs", referrer: "github.com" });
  seedEvent(db, { hash: "new-one", time: NOW, path: "/docs", session: "second-session", country: "GB" });
  seedEvent(db, { hash: "new-two", time: NOW, path: "/docs" });
  const report = await buildReport(env, 7);
  assert.equal(report.enabled, true);
  assert.equal(report.since, "2026-10-13T00:00:00.000Z");
  assert.equal(report.until, "2026-10-19T12:00:00.000Z");
  assert.equal(report.timezone, "UTC");
  assert.deepEqual(report.summary, { uniqueVisitors: 3, visits: 4, pageViews: 5, newVisitors: 2, returningVisitors: 1, excludedEvents: 0 });
  assert.equal(report.daily.length, 7);
  assert.equal(report.daily[0].pageViews, 0);
  assert.equal(report.daily.reduce((sum, day) => sum + day.uniqueVisitors, 0), 5);
  assert.deepEqual(report.pages, [{ path: "/docs", views: 4, visitors: 3 }, { path: "/", views: 1, visitors: 1 }]);
  assert.deepEqual(report.referrers, [{ referrer: "(direct)", visits: 2, visitors: 2 }, { referrer: "github.com", visits: 1, visitors: 1 }, { referrer: "t.co", visits: 1, visitors: 1 }]);
  assert.deepEqual(report.countries, [{ country: "US", visitors: 3 }, { country: "GB", visitors: 1 }]);
  const today = await buildReport(env, 1);
  assert.equal(today.summary.uniqueVisitors, 3);
  assert.equal(today.summary.newVisitors, 1);
  assert.equal(today.summary.returningVisitors, 2);
  assert.ok(today.referrers.some((row) => row.referrer === "t.co"), "session landing referrer survives a midnight boundary");
});

test("retention removes old events and inactive identities while keeping active visitor history", async (t) => {
  const { db, env } = setup(t);
  const old = NOW - 91 * DAY;
  const cutoff = NOW - 90 * DAY;
  seedVisitor(db, "inactive", old);
  seedVisitor(db, "active", old, NOW);
  seedVisitor(db, "boundary", cutoff);
  seedEvent(db, { hash: "inactive", time: old });
  seedEvent(db, { hash: "active", time: old });
  seedEvent(db, { hash: "active", time: NOW });
  seedEvent(db, { hash: "boundary", time: cutoff });
  db.sqlite.prepare("INSERT INTO analytics_exclusions VALUES (?, 'crawler_user_agent', 1)").run("2026-07-20");
  db.sqlite.prepare("INSERT INTO analytics_exclusions VALUES (?, 'crawler_user_agent', 1)").run("2026-10-19");
  await pruneAnalytics(env);
  assert.deepEqual(db.rows("analytics_visitors").map((row) => row.visitor_hash).sort(), ["active", "boundary"]);
  assert.equal(db.rows("analytics_events").length, 2);
  assert.equal(db.rows("analytics_visitors").find((row) => row.visitor_hash === "active").first_seen, old);
  assert.equal(db.rows("analytics_exclusions").length, 1);
  assert.equal((await buildReport(env, 7)).summary.returningVisitors, 1);
});
