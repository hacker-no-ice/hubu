import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { createLocalJWKSet, exportJWK, generateKeyPair, SignJWT } from "jose";
import ts from "typescript";

// Run the exact Worker module without a live Access tenant or network requests.
const source = await readFile(new URL("../worker/analytics-auth.ts", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
}).outputText.replaceAll('from "jose"', `from "${import.meta.resolve("jose")}"`);
const { isAnalyticsAuthorized, verifyAnalyticsToken } = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`);

const env = {
  ANALYTICS_ACCESS_TEAM_DOMAIN: "hubustack-test.cloudflareaccess.com",
  ANALYTICS_ACCESS_AUD: "analytics-test-audience",
};
const issuer = `https://${env.ANALYTICS_ACCESS_TEAM_DOMAIN}`;
const { publicKey, privateKey } = await generateKeyPair("RS256");
const jwk = { ...await exportJWK(publicKey), kid: "access-key", alg: "RS256", use: "sig" };
const keys = createLocalJWKSet({ keys: [jwk] });

async function token(overrides = {}, signingKey = privateKey, algorithm = "RS256") {
  const now = Math.floor(Date.now() / 1_000);
  const claims = {
    type: "app",
    iss: issuer,
    aud: [env.ANALYTICS_ACCESS_AUD],
    sub: "test-person",
    iat: now - 10,
    exp: now + 300,
    ...overrides,
  };
  for (const key of Object.keys(claims)) if (claims[key] === undefined) delete claims[key];
  return new SignJWT(claims).setProtectedHeader({ alg: algorithm, kid: "access-key" }).sign(signingKey);
}

test("accepts a cryptographically valid application token for the configured team and audience", async () => {
  assert.equal(await verifyAnalyticsToken(await token(), env, keys), true);
});

test("denies a valid token issued for another application or team", async () => {
  assert.equal(await verifyAnalyticsToken(await token({ aud: ["another-application"] }), env, keys), false);
  assert.equal(await verifyAnalyticsToken(await token({ iss: "https://another-team.cloudflareaccess.com" }), env, keys), false);
});

test("denies expired tokens, future validity, and future issuance", async () => {
  const now = Math.floor(Date.now() / 1_000);
  assert.equal(await verifyAnalyticsToken(await token({ exp: now - 1 }), env, keys), false);
  assert.equal(await verifyAnalyticsToken(await token({ nbf: now + 60 }), env, keys), false);
  assert.equal(await verifyAnalyticsToken(await token({ iat: now + 60 }), env, keys), false);
});

test("requires expiry, issuance, and a nonempty human subject", async () => {
  for (const overrides of [{ exp: undefined }, { iat: undefined }, { sub: undefined }, { sub: "" }, { sub: " " }]) {
    assert.equal(await verifyAnalyticsToken(await token(overrides), env, keys), false);
  }
  assert.equal(await verifyAnalyticsToken(await token({ type: "org" }), env, keys), false);
});

test("denies signatures from another key and unsupported algorithms", async () => {
  const foreign = await generateKeyPair("RS256");
  assert.equal(await verifyAnalyticsToken(await token({}, foreign.privateKey), env, keys), false);
  const unsupported = await generateKeyPair("PS256");
  assert.equal(await verifyAnalyticsToken(await token({}, unsupported.privateKey, "PS256"), env, keys), false);
});

test("missing configuration and malformed team domains deny without resolving keys", async () => {
  const signed = await token();
  let resolutions = 0;
  const unexpectedResolver = async () => { resolutions++; throw new Error("must not resolve"); };
  for (const config of [
    {},
    { ...env, ANALYTICS_ACCESS_TEAM_DOMAIN: "" },
    { ...env, ANALYTICS_ACCESS_AUD: "" },
    { ...env, ANALYTICS_ACCESS_AUD: " " },
    ...[
      "https://hubustack-test.cloudflareaccess.com",
      "hubustack-test.cloudflareaccess.com/",
      "hubustack-test.cloudflareaccess.com:443",
      "hubustack-test.cloudflareaccess.com?host=evil.example",
      "hubustack-test.cloudflareaccess.com.evil.example",
      "evil.hubustack-test.cloudflareaccess.com",
      "person@hubustack-test.cloudflareaccess.com",
      "-invalid.cloudflareaccess.com",
    ].map((team) => ({ ...env, ANALYTICS_ACCESS_TEAM_DOMAIN: team })),
  ]) {
    assert.equal(await verifyAnalyticsToken(signed, config, unexpectedResolver), false);
  }
  assert.equal(resolutions, 0);
});

test("missing assertions and spoofed identity headers never authorize", async () => {
  const request = new Request("https://hubustack.dev/_analytics", {
    headers: { "cf-access-authenticated-user-email": "owner@example.com" },
  });
  assert.equal(await isAnalyticsAuthorized(request, env), false);
  assert.equal(await isAnalyticsAuthorized(new Request(request, {
    headers: { "cf-access-jwt-assertion": "forged.jwt.signature" },
  }), { ...env, ANALYTICS_ACCESS_AUD: "" }), false);
});

test("key retrieval failures and malformed assertions fail closed", async () => {
  const unavailableKeys = async () => { throw new Error("JWKS unavailable"); };
  assert.equal(await verifyAnalyticsToken(await token(), env, unavailableKeys), false);
  assert.equal(await verifyAnalyticsToken("forged.jwt.signature", env, keys), false);
  assert.equal(await verifyAnalyticsToken("x".repeat(16_385), env, keys), false);
});
