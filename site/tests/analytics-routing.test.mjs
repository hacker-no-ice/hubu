import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { exportJWK, generateKeyPair, SignJWT } from "jose";

async function request(path, options = {}, env = {}) {
  const { default: worker } = await import("../dist/server/index.js");
  return worker.fetch(new Request(`https://hubustack.dev${path}`, options), {
    ASSETS: { fetch: async () => new Response("Not found", { status: 404 }) },
    ...env,
  }, { waitUntil() {}, passThroughOnException() {} });
}

test("dashboard and report deny access before touching storage, including spoofed identity headers", async () => {
  let queries = 0;
  const env = { ANALYTICS_ENABLED: "true", ANALYTICS_DB: { prepare() { queries++; throw new Error("Must not query private data"); } } };
  for (const path of ["/_analytics", "/_analytics/", "/_analytics/report?days=7"]) {
    const response = await request(path, { headers: { "cf-access-authenticated-user-email": "spoof@example.com", "cf-access-jwt-assertion": "not-a-token" } }, env);
    assert.equal(response.status, 403);
    assert.match(response.headers.get("cache-control"), /private, no-store/);
    assert.match(response.headers.get("x-robots-tag"), /noindex/);
    assert.doesNotMatch(await response.text(), /uniqueVisitors|Daily visitors/);
  }
  assert.equal(queries, 0);
});

test("only the beacon and collector are public telemetry endpoints", async () => {
  const beacon = await request("/_telemetry/beacon.js");
  assert.equal(beacon.status, 200);
  assert.match(beacon.headers.get("content-type"), /application\/javascript/);
  assert.match(await beacon.text(), /\/_telemetry\/collect/);
  assert.equal((await request("/_telemetry/report")).status, 404);
  assert.equal((await request("/_analytics/anything")).status, 404);
  assert.equal((await request("/_telemetry/collect")).status, 405);
  assert.equal((await request("/_telemetry/beacon.js", { method: "POST" })).status, 405);
});

test("valid Access sessions reach private reports and a nonce-protected dashboard", async () => {
  const { privateKey, publicKey } = await generateKeyPair("RS256");
  const key = { ...await exportJWK(publicKey), kid: "routing-test", alg: "RS256", use: "sig" };
  const env = { ANALYTICS_ENABLED: "false", ANALYTICS_ACCESS_TEAM_DOMAIN: "routing-test.cloudflareaccess.com", ANALYTICS_ACCESS_AUD: "routing-test-audience" };
  const token = await new SignJWT({ type: "app" }).setProtectedHeader({ alg: "RS256", kid: key.kid })
    .setIssuer(`https://${env.ANALYTICS_ACCESS_TEAM_DOMAIN}`).setAudience(env.ANALYTICS_ACCESS_AUD)
    .setSubject("test-human").setIssuedAt().setExpirationTime("1m").sign(privateKey);
  const options = { headers: { "cf-access-jwt-assertion": token } };
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (url) => {
    assert.equal(String(url), `https://${env.ANALYTICS_ACCESS_TEAM_DOMAIN}/cdn-cgi/access/certs`);
    return Response.json({ keys: [key] });
  };
  try {
    const dashboard = await request("/_analytics", options, env);
    assert.equal(dashboard.status, 200);
    const html = await dashboard.text();
    const nonce = html.match(/<script nonce="([^"]+)"/)[1];
    assert.ok(html.includes(`<style nonce="${nonce}">`));
    assert.ok(dashboard.headers.get("content-security-policy").includes(`script-src 'nonce-${nonce}'`));
    assert.match(dashboard.headers.get("cache-control"), /private, no-store/);
    const report = await request("/_analytics/report?days=30", options, env);
    assert.equal(report.status, 200);
    assert.equal((await report.json()).daily.length, 30);
    assert.match(report.headers.get("cache-control"), /private, no-store/);
    assert.equal((await request("/_analytics/report?days=0", options, env)).status, 400);
    assert.equal((await request("/_analytics", { ...options, method: "POST" }, env)).status, 405);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("disabled collection accepts no identity or writes and does not impact public delivery", async () => {
  const response = await request("/_telemetry/collect", { method: "POST", headers: { origin: "https://hubustack.dev", "content-type": "application/json" }, body: "{}" }, { ANALYTICS_ENABLED: "false", ANALYTICS_ORIGIN: "https://hubustack.dev" });
  assert.equal(response.status, 204);
  assert.equal(response.headers.get("set-cookie"), null);
  assert.equal(response.headers.get("cache-control"), "no-store");
});

test("public pages and static architecture guides load the telemetry beacon and explain privacy", async () => {
  const html = await (await request("/privacy")).text();
  assert.match(html, /Privacy &amp; analytics/);
  assert.match(html, /Exclude this browser/);
  assert.match(html, /Global Privacy Control/);
  assert.match(html, /src="\/_telemetry\/beacon.js"/);
  for (const path of ["../dist/client/architecture/index.html", "../dist/client/architecture/internal/index.html"]) {
    assert.match(await readFile(new URL(path, import.meta.url), "utf8"), /src="\/_telemetry\/beacon.js"/);
  }
});
