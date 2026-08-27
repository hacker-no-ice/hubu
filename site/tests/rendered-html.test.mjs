import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

async function render(pathname = "/") {
  const workerUrl = new URL("../dist/server/index.js", import.meta.url);
  workerUrl.searchParams.set("test", `${process.pid}-${Date.now()}-${pathname}`);
  const { default: worker } = await import(workerUrl.href);
  return worker.fetch(new Request(`http://localhost${pathname}`, { headers: { accept: "text/html" } }), {
    ASSETS: { fetch: async () => new Response("Not found", { status: 404 }) },
  }, { waitUntil() {}, passThroughOnException() {} });
}

test("server-renders the Hubu documentation home", async () => {
  const response = await render();
  assert.equal(response.status, 200);
  const html = await response.text();
  assert.match(html, /Governed spend/);
  assert.match(html, /Experimental and local-first/);
  assert.match(html, /hubu stack start/);
  assert.match(html, /MANAGED LIFECYCLE/);
  assert.equal(html.match(/src="\/brand\/hubu-wordmark\.png"/g)?.length, 2);
  assert.match(html, /alt="Hubu"/);
  assert.match(html, /aria-label="Hubu documentation home"/);
  assert.match(html, /og-wordmark\.png/);
  assert.doesNotMatch(html, /not on main yet/i);
  assert.doesNotMatch(html, /codex-preview|react-loading-skeleton/i);
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
  assert.match(html, /href="https:\/\/hubu-docs\.water-no-ice\.chatgpt\.site\/configuration\/local-stack\/v1\/"/);
  assert.doesNotMatch(html, /Component ownership|Clean-environment acceptance canary|Runtime and recovery boundaries/);
  assert.doesNotMatch(html, /not on main yet/i);
  assert.match(html, /On this page/);
  assert.match(html, /src="\/brand\/hubu-wordmark\.png"/);
  assert.match(html, /aria-label="Hubu documentation home"/);
});

test("publishes the versioned local-stack configuration reference at stable public routes", async () => {
  const landing = await render("/configuration/local-stack/v1");
  assert.equal(landing.status, 200);
  const landingHtml = await landing.text();
  assert.match(landingHtml, /Local stack configuration reference/);
  assert.match(landingHtml, /Value-source labels/);
  assert.match(landingHtml, /provider-disabled example/i);
  assert.match(landingHtml, /href="\/configuration\/local-stack\/v1\/stack-toml"/);

  const providers = await render("/configuration/local-stack/v1/providers-toml");
  assert.equal(providers.status, 200);
  const providersHtml = await providers.text();
  assert.match(providersHtml, /I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND/);
  assert.match(providersHtml, /rate_numerator_minor/);
  assert.match(providersHtml, /provider_config_version/);
  assert.match(providersHtml, /1\.\.=270000/);
  assert.match(providersHtml, /only currently accepted value is <code>0<\/code>/);
  assert.match(providersHtml, /required and must contain at least one host for <code>flux2_api<\/code> and <code>ideogram_image<\/code>/);
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
      "targets.settings.config.project", "targets.settings.config.location",
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

test("keeps component drill-down diagrams and resettable architecture playback", async () => {
  const [html, script] = await Promise.all([
    readFile(new URL("../architecture/index.html", import.meta.url), "utf8"),
    readFile(new URL("../architecture/architecture.js", import.meta.url), "utf8"),
  ]);
  assert.match(html, /id="detail-diagram"/);
  assert.match(html, /data-component="registration"/);
  assert.match(html, /data-component="policy"/);
  assert.match(html, /data-component="budgets"/);
  assert.match(html, /data-component="persistence"/);
  assert.match(script, /Identity \+ registration/);
  assert.match(script, /function stopPlayback\(\)/);
  assert.match(script, /setAttribute\("aria-pressed", "false"\)/);
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
