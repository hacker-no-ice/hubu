import assert from "node:assert/strict";
import { access, readFile } from "node:fs/promises";
import test from "node:test";
import { waitForRevision } from "../scripts/verify-production-revision.mjs";

async function render(pathname = "/", origin = "http://localhost") {
  const workerUrl = new URL("../dist/server/index.js", import.meta.url);
  workerUrl.searchParams.set("test", `${process.pid}-${Date.now()}-${origin}-${pathname}`);
  const { default: worker } = await import(workerUrl.href);
  return worker.fetch(new Request(new URL(pathname, origin), { headers: { accept: "text/html" } }), {
    ASSETS: { fetch: async () => new Response("Not found", { status: 404 }) },
  }, { waitUntil() {}, passThroughOnException() {} });
}

test("server-renders the Hubu documentation home", async () => {
  const response = await render();
  assert.equal(response.status, 200);
  assert.equal(
    response.headers.get("x-hubustack-revision"),
    process.env.HUBUSTACK_SOURCE_REVISION ?? "local",
  );
  const html = await response.text();
  assert.match(html, /Governed spend/);
  assert.match(html, /Experimental and local-first/);
  assert.match(html, /Hubu governs AI-agent spend/);
  assert.match(html, /Gongbu executes only Hubu-authorized provider work/);
  assert.match(html, /The ecosystem they form is the product/);
  assert.match(html, /Initialize a profile/);
  assert.match(html, /hubu stack select --profile/);
  assert.match(html, /hubu stack start/);
  assert.match(html, /Connect your favorite agent harness/);
  assert.match(html, /authorize → execute → settle, release, or reconcile/);
  assert.doesNotMatch(html, /Hubu gives humans|v4\.2|One command from profile to running|MANAGED LIFECYCLE/);
  assert.doesNotMatch(html, /authorize → execute → submit/);
  assert.equal(html.match(/src="\/brand\/hubu-wordmark\.svg"/g)?.length, 2);
  assert.match(html, /alt="Hubu"/);
  assert.match(html, /aria-label="Hubu documentation home"/);
  assert.match(html, /og-wordmark\.png/);
  assert.match(html, /https:\/\/hubustack\.dev\/og-wordmark\.png/);
  assert.doesNotMatch(html, /not on main yet/i);
  assert.doesNotMatch(html, /codex-preview|react-loading-skeleton/i);
});

test("publishes the scalable Hubu wordmark", async () => {
  const svg = await readFile(new URL("../public/brand/hubu-wordmark.svg", import.meta.url), "utf8");
  assert.match(svg, /viewBox="269 286 1168 376"/);
  assert.match(svg, /linearGradient id="hubu-gradient"/);
  assert.match(svg, /#9697ff/);
  assert.match(svg, /#71d7e8/);
});

test("reports the exact source revision for production verification", async () => {
  const response = await render("/.well-known/hubustack-revision");
  const expectedRevision = process.env.HUBUSTACK_SOURCE_REVISION ?? "local";
  assert.equal(response.status, 200);
  assert.equal(response.headers.get("cache-control"), "no-store");
  assert.equal(response.headers.get("content-type"), "text/plain; charset=utf-8");
  assert.equal(response.headers.get("x-hubustack-revision"), expectedRevision);
  assert.equal(await response.text(), expectedRevision);
  await assert.rejects(
    access(new URL("../dist/client/.well-known/hubustack-revision", import.meta.url)),
    { code: "ENOENT" },
  );
});

test("waits for the deployed revision to replace a stale response", async () => {
  const expectedRevision = "expected-revision";
  const responses = [
    new Response("stale-revision"),
    new Response(expectedRevision),
  ];
  let attempts = 0;

  await waitForRevision({
    endpoint: "https://hubustack.dev/.well-known/hubustack-revision",
    expectedRevision,
    attempts: 2,
    delayMs: 0,
    fetchImpl: async () => {
      attempts += 1;
      return responses.shift();
    },
    sleep: async () => {},
  });

  assert.equal(attempts, 2);
});

test("fails when production never reports the expected revision", async () => {
  await assert.rejects(
    waitForRevision({
      endpoint: "https://hubustack.dev/.well-known/hubustack-revision",
      expectedRevision: "expected-revision",
      attempts: 2,
      delayMs: 0,
      fetchImpl: async () => new Response("stale-revision"),
      sleep: async () => {},
    }),
    /Expected production revision expected-revision after 2 attempts; last result: stale-revision/,
  );
});

test("redirects the legacy Sites hostname to the canonical domain", async () => {
  const response = await render(
    "/docs/overview?source=legacy",
    "https://hubu-docs.water-no-ice.chatgpt.site",
  );
  assert.equal(response.status, 308);
  assert.equal(response.headers.get("location"), "https://hubustack.dev/docs/overview?source=legacy");
});

test("renders the command-focused local stack quick start", async () => {
  const response = await render("/docs/local-stack");
  assert.equal(response.status, 200);
  const html = await response.text();
  assert.match(html, /Local stack quick start/);
  assert.match(html, /hubu stack init/);
  assert.match(html, /stack doctor/);
  assert.match(html, /stack start/);
  assert.match(html, /stack status/);
  assert.match(html, /hubu init codex/);
  assert.match(html, /href="https:\/\/hubustack\.dev\/configuration\/local-stack\/v1\/"/);
  assert.doesNotMatch(html, /Component ownership|Clean-environment acceptance canary|Runtime and recovery boundaries/);
  assert.doesNotMatch(html, /not on main yet/i);
  assert.match(html, /On this page/);
  assert.match(html, /src="\/brand\/hubu-wordmark\.svg"/);
  assert.match(html, /aria-label="Hubu documentation home"/);
});

test("publishes the versioned local-stack configuration reference at stable public routes", async () => {
  const landing = await render("/configuration/local-stack/v1");
  assert.equal(landing.status, 200);
  const landingHtml = await landing.text();
  assert.match(landingHtml, /Local stack configuration reference/);
  assert.match(landingHtml, /Value-source labels/);
  assert.match(landingHtml, /stack init --mode sandbox/i);
  assert.match(landingHtml, /href="\/configuration\/local-stack\/v1\/stack-toml"/);

  const providers = await render("/configuration/local-stack/v1/providers-toml");
  assert.equal(providers.status, 200);
  const providersHtml = await providers.text();
  assert.match(providersHtml, /I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND/);
  assert.match(providersHtml, /rate_numerator_minor/);
  assert.match(providersHtml, /provider_config_version/);
  assert.match(providersHtml, /1\.\.=270000/);
  assert.match(providersHtml, /only currently accepted value is <code>0<\/code>/);
  assert.match(providersHtml, /required and must contain at least\s+one host for <code>ideogram_image<\/code>/);
  assert.match(providersHtml, /delivery\.&lt;region&gt;\.bfl\.ai/);
});

test("documents every schema-v1 local-stack source field", async () => {
  const references = [
    ["../../docs/configuration/local-stack/v1/stack-toml.md", [
      "schema_version", "allow_development_builds", "binaries.hubu", "binaries.hubu_server",
      "binaries.gongbu_server", "binaries.hubu_unified_mcp", "identity.account_id",
      "identity.agent_id", "hubu.ownership", "hubu.endpoint", "hubu.listen",
      "hubu.database_path", "hubu.log_file", "gongbu.ownership", "gongbu.endpoint",
      "gongbu.listen", "gongbu.database_path", "gongbu.artifact_root", "gongbu.log_file",
      "temporal.mode", "temporal.binary_path", "temporal.expected_cli_version",
      "temporal.data_path", "temporal.rpc_port", "temporal.ui_port", "temporal.address",
      "temporal.namespace", "temporal.task_queue", "temporal.ui_url",
      "runtime.hubu_startup_policy", "runtime.hubu_startup_timeout_ms",
      "runtime.recovery_delays_seconds", "runtime.temporal_startup_timeout_ms",
      "runtime.dependency_check_interval_ms", "runtime.worker_drain_timeout_ms",
      "runtime.max_artifacts_per_execution", "runtime.max_encoded_bytes",
      "runtime.max_decoded_bytes", "runtime.max_width", "runtime.max_height",
      "runtime.log_level", "runtime.log_format",
    ]],
    ["../../docs/configuration/local-stack/v1/credentials-toml.md", [
      "schema_version", "files.hubu_auth", "files.hubu_approval",
      "files.hubu_reconciliation", "files.gongbu_caller", "opaque.<key>.service",
      "opaque.<key>.account", "opaque.gongbu_hubu", "opaque.gongbu_caller",
    ]],
    ["../../docs/configuration/local-stack/v1/providers-toml.md", [
      "schema_version", "mode", "catalog_version", "maximum_spend_minor",
      "live_spend_acknowledgement", "targets.provider_config_version",
      "targets.workload_type", "targets.provider", "targets.adapter", "targets.model",
      "targets.credential", "targets.active", "targets.execution_enabled", "targets.settings",
      "targets.settings.type", "targets.settings.config.endpoint",
      "targets.settings.config.api_version", "targets.settings.config.timeout_ms",
      "targets.settings.config.max_retries", "targets.settings.config.headers",
      "targets.settings.config.approved_artifact_hosts",
      "targets.settings.config.poll_interval_ms", "targets.settings.config.idempotency_header",
      "pricing_rules.rule_id", "pricing_rules.provider", "pricing_rules.model",
      "pricing_rules.currency", "pricing_rules.selector", "pricing_rules.selector.image_size",
      "pricing_rules.components", "pricing_rules.components.unit",
      "pricing_rules.components.rate_numerator_minor",
      "pricing_rules.components.rate_denominator",
    ]],
  ];

  for (const [path, fields] of references) {
    const source = await readFile(new URL(path, import.meta.url), "utf8");
    for (const field of fields) assert.match(source, new RegExp("### `" + field.replaceAll(".", "\\.") + "`"), `${path} is missing ${field}`);
  }
});

test("publishes one live-provider operations entry point", async () => {
  const response = await render("/docs/operations/live-providers");
  assert.equal(response.status, 200);
  const html = await response.text();
  assert.match(html, /Live provider operations/);
  assert.match(html, /Gemini Developer API/);
  assert.match(html, /FLUX\.2 Pro/);
  assert.match(html, /Submission, retry, and reconciliation/);
  assert.doesNotMatch(html, /Vertex AI/);

  const compatibility = await render("/docs/operations/live-provider-testing");
  assert.equal(compatibility.status, 200);
  const compatibilityHtml = await compatibility.text();
  assert.match(compatibilityHtml, /href="\/docs\/operations\/live-providers"/);
});

test("keeps managed credential locations out of the first-run profile", async () => {
  const [examples, credentials, localStack, readme] = await Promise.all([
    readFile(new URL("../../docs/configuration/local-stack/v1/examples.md", import.meta.url), "utf8"),
    readFile(new URL("../../docs/configuration/local-stack/v1/credentials-toml.md", import.meta.url), "utf8"),
    readFile(new URL("../../docs/local-stack.md", import.meta.url), "utf8"),
    readFile(new URL("../../README.md", import.meta.url), "utf8"),
  ]);
  assert.doesNotMatch(examples, /^\[files\]$/m);
  assert.doesNotMatch(examples, /^\[opaque\.gongbu_(hubu|caller)\]$/m);
  assert.doesNotMatch(`${localStack}\n${readme}`, /temporary Hubu process|pre-provision(?:ing)? workaround/i);
  assert.match(credentials, /final managed `hubu-server` creates or\s+reuses those capabilities/i);
  assert.match(credentials, /Gongbu-owned bootstrap/i);
});

test("promotes complete mode-specific stack examples", async () => {
  const [examples, navigation, examplesResponse, credentialsResponse] = await Promise.all([
    readFile(new URL("../../docs/configuration/local-stack/v1/examples.md", import.meta.url), "utf8"),
    readFile(new URL("../app/lib/docs.ts", import.meta.url), "utf8"),
    render("/configuration/local-stack/v1/examples"),
    render("/configuration/local-stack/v1/credentials-toml"),
  ]);
  const [examplesHtml, credentialsHtml] = await Promise.all([
    examplesResponse.text(),
    credentialsResponse.text(),
  ]);

  assert.match(examples, /## Sandbox: complete stack without live spend/);
  assert.match(examples, /## Hubu-only: governance without an execution plane/);
  assert.match(examples, /## Live: Gemini Developer API and FLUX\.2/);
  assert.match(examples, /service` maps to the Keychain Access \*\*Where\*\* field/);
  assert.match(examples, /account` maps to the Keychain Access \*\*Account\*\* field/);
  assert.match(examples, /matching \*\*Name\*\* alone is insufficient/);
  assert.match(examples, /find-generic-password[\s\S]*>\/dev\/null 2>&1/);
  assert.match(examples, /hubu\.gemini-3\.1-flash-lite-image\.text-to-image\/v1/);
  assert.match(examples, /hubu\.gemini-3\.1-flash-image\.text-to-image\/v1/);
  assert.match(examples, /hubu\.flux-2-pro\.text-to-image\/v1/);
  assert.match(examples, /`hubu stack doctor` is the authoritative validation path/);
  assert.match(examples, /production_validated = false` until a generation has been\s+rendered/);
  assert.match(examples, /hubu stack render[\s\S]*hubu stack doctor/);
  assert.match(examples, /## Keep sandbox and live profiles separate/);
  assert.doesNotMatch(examples, /Provider-disabled local-stack variation/);
  assert.doesNotMatch(examples, /Live-profile review checklist/);
  assert.doesNotMatch(examples, /External-service variations/);
  assert.match(navigation, /Start here[^\n]*Complete stack examples/);
  assert.match(examplesHtml, /id="edit-credentials-toml"/);
  assert.match(credentialsHtml, /href="\/configuration\/local-stack\/v1\/examples#edit-credentials-toml"/);
});

test("renders the concise canonical overview", async () => {
  const response = await render("/docs/overview");
  assert.equal(response.status, 200);
  const html = await response.text();
  assert.match(html, /Why Hubu and Gongbu/);
  assert.match(html, /Hubu governs resources\. Gongbu performs the work\./);
  assert.match(html, /Experimental and local-first/);
  assert.doesNotMatch(html, /What Hubu Does Today|Crates|Local Developer Tools/);
});

test("uses GitHub tree URLs for repository directory links", async () => {
  const response = await render("/docs/spend-lifecycle");
  assert.equal(response.status, 200);
  const html = await response.text();
  assert.match(html, /github\.com\/hacker-no-ice\/hubu\/tree\/main\/crates\/hubu-core/);
  assert.doesNotMatch(html, /github\.com\/hacker-no-ice\/hubu\/blob\/main\/crates\/hubu-core["#]/);
});

test("uses document navigation for deployed subpage reliability", async () => {
  const sources = await Promise.all([
    readFile(new URL("../app/page.tsx", import.meta.url), "utf8"),
    readFile(new URL("../app/components/DocsShell.tsx", import.meta.url), "utf8"),
    readFile(new URL("../app/components/Search.tsx", import.meta.url), "utf8"),
  ]);
  assert.doesNotMatch(sources.join("\n"), /next\/link|<Link\b/);
});

test("publishes the high-level topology and four focused component drills", async () => {
  const [html, script, styles, publishedHtml] = await Promise.all([
    readFile(new URL("../architecture/index.html", import.meta.url), "utf8"),
    readFile(new URL("../architecture/architecture.js", import.meta.url), "utf8"),
    readFile(new URL("../architecture/architecture.css", import.meta.url), "utf8"),
    readFile(new URL("../public/architecture/index.html", import.meta.url), "utf8"),
  ]);
  assert.equal(publishedHtml, html);
  assert.match(html, /src="\/brand\/hubu-wordmark\.svg"/);
  assert.match(html, />\/ architecture</);
  assert.match(html, /HIGH-LEVEL TOPOLOGY/);
  assert.match(html, /Hubu governs every billable operation/);
  assert.doesNotMatch(html, /Hubu governs every request/);
  assert.match(html, /AGENT ADAPTER PROCESS/);
  assert.match(html, /Operation registry SQLite/);
  assert.match(html, /HUBU CLIENT/);
  assert.match(html, /GONGBU CLIENT/);
  assert.match(html, /Hubu HTTP API/);
  assert.match(html, /Hubu SQLite/);
  assert.match(html, /Execution API/);
  assert.match(html, /Temporal worker/);
  assert.match(html, /Provider catalog \+ execution/);
  assert.match(html, /catalog · validate · persist · schedule/);
  assert.match(html, /submit once \+ resume poll/);
  assert.match(html, /Gongbu SQLite/);
  assert.match(html, /executions · checkpoints · receipts/);
  assert.match(html, /Provider APIs/);
  assert.match(html, /id="detail-diagram"/);
  assert.match(html, /id="tab-registration"[^>]+aria-controls="component-panel"/);
  assert.match(html, /id="component-panel"[^>]+role="tabpanel"[^>]+aria-labelledby="tab-registration"/);
  assert.deepEqual(
    [...html.matchAll(/role="tab"[^>]+data-component="([^"]+)"/g)].map((match) => match[1]),
    ["registration", "policy", "budgets", "executor"],
  );
  assert.match(script, /Owner user/);
  assert.match(script, /Agent identity/);
  assert.match(script, /Evaluate every rule, then decide/);
  assert.match(script, /deny > needs approval > allow > default/);
  assert.match(script, /Version, reserve, then finalize/);
  assert.match(script, /settle · release · reconcile/);
  assert.match(script, /Execute only approved work/);
  assert.match(script, /Provider contract validation/);
  assert.match(script, /generation POST once/);
  assert.match(script, /same provider operation · read-only polling/);
  assert.match(script, /never resubmit after an ambiguous post-transmission interruption/);
  assert.match(script, /setAttribute\("aria-labelledby", selectedTab\.id\)/);
  assert.match(script, /ArrowRight/);
  assert.match(script, /ArrowLeft/);
  assert.match(script, /event\.key === "Home"/);
  assert.match(script, /event\.key === "End"/);
  assert.match(script, /tab\.tabIndex = selected \? 0 : -1/);
  assert.match(styles, /@media \(max-width: 640px\)/);
  assert.match(styles, /\.detail-diagram\.is-relations \{ grid-template-columns: 1fr; \}/);
  assert.doesNotMatch(script, /asking Hubu to settle, release, or reconcile/);
  assert.doesNotMatch(html, /admin \+ lifecycle → Hubu/);
  assert.doesNotMatch(`${html}\n${script}`, /v4\.2|Unified MCP <code>|data-stage-button|play-flow|setInterval/);
});

test("publishes the original engineering architecture explorer separately", async () => {
  const [source, published] = await Promise.all([
    readFile(new URL("../../architecture/index.html", import.meta.url), "utf8"),
    readFile(new URL("../public/architecture/internal/index.html", import.meta.url), "utf8"),
  ]);
  assert.match(source, /Agent Spend Control Plane/);
  assert.match(source, /Major Components/);
  assert.equal(published, source);
});

test("builds the direct hubustack.dev Cloudflare deployment target", async () => {
  const config = JSON.parse(
    await readFile(new URL("../dist/server/wrangler.json", import.meta.url), "utf8"),
  );
  assert.equal(config.name, "hubustack-docs");
  assert.equal(config.workers_dev, false);
  assert.equal(config.preview_urls, false);
  assert.deepEqual(config.routes, [
    { pattern: "hubustack.dev", custom_domain: true },
  ]);
  assert.equal(config.assets.binding, "ASSETS");
  assert.equal(config.assets.directory, "../client");
  assert.equal(config.assets.run_worker_first, undefined);
  assert.deepEqual(config.images, { binding: "IMAGES" });
});
