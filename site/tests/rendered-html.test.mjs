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
  assert.match(html, /Agents get a budget, never your keys\./);
  assert.doesNotMatch(html, /The ecosystem they form is the product/);
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

test("embeds the introduction below the hero without autoplay", async () => {
  const html = await (await render()).text();
  assert.match(html, /Meet Hubu in 3 min/);
  const iframe = html.match(/<iframe\b[^>]*><\/iframe>/)?.[0];
  assert.ok(iframe);
  assert.match(iframe, /src="https:\/\/www.youtube-nocookie.com\/embed\/ufEgYjmxKWM"/);
  assert.match(iframe, /title="Introducing Hubu: bounded spending power for AI agents"/);
  assert.match(iframe, /loading="lazy"/);
  assert.match(iframe, /referrerPolicy="strict-origin-when-cross-origin"/i);
  assert.doesNotMatch(iframe, /autoplay/);
  assert.match(html, /href="https:\/\/youtu.be\/ufEgYjmxKWM"/);
  assert.ok(html.indexOf('class="hero"') < html.indexOf('id="intro-video-title"'));
  assert.ok(html.indexOf('id="intro-video-title"') < html.indexOf('class="warning-band"'));
});

test("home page walks through an illustrative governed image request", async () => {
  const html = await (await render()).text();
  assert.match(html, /<ol class="example-flow">/);
  assert.match(html, /Agent asks for an image/);
  assert.match(html, /Hubu checks policy and reserves \$0\.05/);
  assert.match(html, /Gongbu calls the provider/);
  assert.match(html, /reports a \$0\.03 receipt/);
  assert.match(html, /Hubu settles \$0\.03 and releases \$0\.02/);
  assert.match(html, /Illustrative amounts\./);
  assert.ok(html.indexOf('class="hero"') < html.indexOf('id="worked-example-title"'));
  assert.ok(html.indexOf('id="worked-example-title"') < html.indexOf('id="intro-video-title"'));
});

test("home page states that Gongbu is optional and links what works today", async () => {
  const html = await (await render()).text();
  assert.match(html, /Gongbu is the first-party executor, not a requirement/);
  assert.match(html, /<code>hubu-only<\/code>/);
  assert.match(html, /href="\/docs\/external-executor">bring your own executor/);
  assert.match(html, /<section class="works-today" aria-labelledby="works-today-title">/);
  assert.match(html, /<strong>macOS<\/strong> source install from a release tag/);
  assert.match(html, /<strong>Gemini and FLUX\.2 Pro<\/strong> live providers/);
  assert.match(html, /<strong>Sandbox<\/strong> with no provider credentials/);
  assert.match(html, /href="\/docs\/overview#what-works-today"/);
  assert.ok(html.indexOf('class="warning-band"') < html.indexOf('class="works-today"'));
});

test("home page starts the stack steps with an install step and routes topic cards to first steps", async () => {
  const html = await (await render()).text();
  assert.match(html, /<span>00<\/span><div><h3>Install from a release tag<\/h3><a class="step-link" href="\/docs\/local-stack#install">/);
  assert.match(html, /Build the four binaries from an exact release tag on macOS\./);
  assert.ok(html.indexOf("Install from a release tag") < html.indexOf("Initialize a profile"));
  const topics = html.match(/<div class="topic-grid">[\s\S]*?<\/div>/)?.[0];
  assert.ok(topics);
  assert.deepEqual(
    [...topics.matchAll(/<a href="([^"]+)"><small>[^<]+<\/small><h3>([^<]+)<\/h3>/g)].map(([, href, title]) => [href, title]),
    [
      ["/docs/local-stack", "Quick start"],
      ["/docs/policy-engine", "Write a policy"],
      ["/docs/unified-mcp#setup", "Connect your agent"],
      ["/docs/external-executor", "Use your own executor"],
    ],
  );
  assert.match(topics, /From install to your first governed request\./);
  assert.doesNotMatch(topics, /Register an agent|Trace a spend|Understand Gongbu|Use unified MCP/);
});

test("home page offers copyable commands, a mobile menu, and anchored warning", async () => {
  const html = await (await render()).text();
  assert.doesNotMatch(html, /--profile …|--stack-profile …/);
  assert.match(html, /hubu stack init --mode sandbox --profile &quot;\$HOME\/hubu-sandbox&quot;/);
  assert.equal(html.match(/<button class="copy-code"/g)?.length, 5);
  assert.match(html, /<details class="site-menu"><summary>Menu<\/summary>/);
  assert.match(html, /href="\/docs\/overview#project-status"/);
  assert.match(html, /<link rel="icon" href="\/favicon.svg" type="image\/svg\+xml"/);
  const favicon = await readFile(new URL("../public/favicon.svg", import.meta.url), "utf8");
  assert.match(favicon, /linearGradient id="hubu-icon-gradient"/);
});

test("documentation code blocks carry progressive copy buttons", async () => {
  const html = await (await render("/docs/local-stack")).text();
  assert.match(html, /<div class="code-block"><pre><code[^>]*>[\s\S]*?<\/code><\/pre>\s*<button class="copy-code" type="button" data-copy-code hidden>Copy<\/button><\/div>/);
  assert.match(html, /GitHub repository/);
  const overview = await (await render("/docs/overview")).text();
  assert.match(overview, /id="project-status"/);
});

test("publishes per-page canonical and share metadata", async () => {
  const home = await (await render()).text();
  assert.match(home, /<link rel="canonical" href="https:\/\/hubustack.dev"/);
  assert.match(home, /<meta property="og:title" content="Hubu Docs — Governed spend for AI agents"/);

  const html = await (await render("/docs/local-stack")).text();
  assert.match(html, /<link rel="canonical" href="https:\/\/hubustack.dev\/docs\/local-stack"/);
  assert.match(html, /<meta property="og:title" content="Local stack quick start · Hubu Docs"/);
  assert.match(html, /<meta property="og:url" content="https:\/\/hubustack.dev\/docs\/local-stack"/);
  assert.match(html, /<meta name="twitter:title" content="Local stack quick start · Hubu Docs"/);
  assert.match(html, /<meta property="og:image" content="https:\/\/hubustack.dev\/og-wordmark.png"/);
  const description = html.match(/<meta name="description" content="([^"]*)"/)?.[1];
  assert.ok(description && description.length <= 160);
  assert.match(description, /\w…$/);
  assert.match(html, /← Previous<\/small><strong>Overview<\/strong>/);

  const missing = await render("/docs/nope");
  assert.equal(missing.status, 404);
  assert.match(await missing.text(), /<title>Page not found · Hubu Docs<\/title>/);
});

test("publishes a sitemap of every documentation route", async () => {
  const [sitemap, robots] = await Promise.all([
    readFile(new URL("../dist/client/sitemap.xml", import.meta.url), "utf8"),
    readFile(new URL("../dist/client/robots.txt", import.meta.url), "utf8"),
  ]);
  assert.match(robots, /^Sitemap: https:\/\/hubustack.dev\/sitemap.xml$/m);
  for (const pathname of ["/", "/architecture/", "/docs/overview", "/docs/local-stack", "/configuration/local-stack/v1", "/configuration/local-stack/v1/stack-toml"]) {
    assert.ok(sitemap.includes(`<loc>https://hubustack.dev${pathname}</loc>`), pathname);
  }
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
  assert.match(html, /<h2 id="make-your-first-governed-request">Make your first governed request/);
  assert.match(html, /hubu_submit_governed_execution/);
  assert.match(html, /hubu spend authorizations/);
  assert.match(html, /href="\/docs\/operations\/managing-a-stack"/);
  assert.doesNotMatch(html, /Terminal color and automation|allow_development_builds|NO_COLOR|hubu stack rollback/);
  assert.match(html, /On this page/);
  assert.match(html, /src="\/brand\/hubu-wordmark\.svg"/);
  assert.match(html, /aria-label="Hubu documentation home"/);
});

test("renders the local stack management runbook", async () => {
  const response = await render("/docs/operations/managing-a-stack");
  assert.equal(response.status, 200);
  const html = await response.text();
  assert.match(html, /Managing a local stack/);
  for (const id of ["routine-operations", "apply-a-configuration-change", "roll-back", "terminal-color-and-automation", "managed-logs"]) {
    assert.match(html, new RegExp(`<h2 id="${id}">`), id);
  }
  assert.match(html, /hubu stack rollback --generation/);
  assert.match(html, /href="\/docs\/local-stack"/);
  const navigation = await readFile(new URL("../app/lib/docs.ts", import.meta.url), "utf8");
  assert.match(navigation, /\["CLI administration", "cli"\], \["Managing a local stack", "operations\/managing-a-stack"\]/);
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

  const legacy = await render("/docs/operations/live-provider-testing?utm=old");
  assert.equal(legacy.status, 308);
  assert.equal(legacy.headers.get("location"), "http://localhost/docs/operations/live-providers?utm=old");
  assert.ok(legacy.headers.get("x-hubustack-revision"));
  assert.ok(legacy.headers.get("strict-transport-security"));
});

// Source paths of docs that stay in the repository but are not published. Keep
// in sync with unpublishedSources in scripts/generate-content.mjs.
const unpublishedDocs = [
  ["docs/budget-architecture.md", "budget-architecture"],
  ["docs/ledger-accounting.md", "ledger-accounting"],
  ["docs/operations/repository-security.md", "operations/repository-security"],
  ["docs/operations/benchmarking.md", "operations/benchmarking"],
  ["docs/operations/gongbu-sandbox.md", "operations/gongbu-sandbox"],
  ["docs/operations/live-provider-testing.md", "operations/live-provider-testing"],
  ["docs/operations/publishing-releases.md", "operations/publishing-releases"],
];

async function generatedDocuments() {
  const source = await readFile(new URL("../app/generated-docs.ts", import.meta.url), "utf8");
  const json = source.replace(/^[\s\S]*?export const documents = /, "").replace(/ as const;\s*$/, "");
  return JSON.parse(json);
}

test("every published document appears in the navigation", async () => {
  const [documents, docsSource] = await Promise.all([
    generatedDocuments(),
    readFile(new URL("../app/lib/docs.ts", import.meta.url), "utf8"),
  ]);
  const navStart = docsSource.indexOf("export const navGroups");
  const navBlock = docsSource.slice(navStart, docsSource.indexOf("] as const;", navStart));
  const navSlugs = [...navBlock.matchAll(/\["[^"\]]+", "([^"\]]+)"\]/g)].map((match) => match[1]);
  assert.ok(navSlugs.length > 10);
  assert.equal(new Set(navSlugs).size, navSlugs.length, "navigation lists a document twice");
  const published = documents.map((document) => document.slug);
  for (const slug of published) assert.ok(navSlugs.includes(slug), `${slug} is published but missing from navGroups`);
  for (const slug of navSlugs) assert.ok(published.includes(slug), `${slug} is in navGroups but not published`);
});

test("does not publish maintainer-only documents", async () => {
  const [documents, sitemap] = await Promise.all([
    generatedDocuments(),
    readFile(new URL("../dist/client/sitemap.xml", import.meta.url), "utf8"),
  ]);
  for (const [sourcePath, slug] of unpublishedDocs) {
    assert.ok(!documents.some((document) => document.slug === slug || document.sourcePath === sourcePath), `${slug} is generated`);
    assert.ok(!sitemap.includes(`/docs/${slug}</loc>`), `${slug} is in the sitemap`);
    if (slug !== "operations/live-provider-testing") {
      const response = await render(`/docs/${slug}`);
      assert.equal(response.status, 404, slug);
    }
  }
});

test("links from published pages to unpublished documents use GitHub blob URLs", async () => {
  const response = await render("/docs/ledger-history");
  assert.equal(response.status, 200);
  const html = await response.text();
  assert.match(html, /href="https:\/\/github\.com\/hacker-no-ice\/hubu\/blob\/main\/docs\/ledger-accounting\.md"/);
  assert.doesNotMatch(html, /href="\/docs\/ledger-accounting"/);
  const lifecycle = await (await render("/docs/spend-lifecycle")).text();
  assert.match(lifecycle, /href="https:\/\/github\.com\/hacker-no-ice\/hubu\/blob\/main\/docs\/budget-architecture\.md"/);
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
  assert.match(localStack, /needs no provider\s+credentials/);
  assert.doesNotMatch(localStack, /^\[files\]$|hubu_auth|\.auth-token/m);
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
  assert.equal((examples.match(/hubu stack select --profile/g) ?? []).length, 3);
  assert.doesNotMatch(examples, /security find-generic-password/);
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
  assert.match(examplesHtml, /id="edit-credentialstoml"/);
  assert.match(credentialsHtml, /href="\/configuration\/local-stack\/v1\/examples#edit-credentialstoml"/);
});

test("renders the concise canonical overview", async () => {
  const response = await render("/docs/overview");
  assert.equal(response.status, 200);
  const html = await response.text();
  assert.match(html, /Why Hubu and Gongbu/);
  assert.match(html, /Hubu governs resources\. Gongbu performs the work\./);
  assert.match(html, /Experimental and local-first/);
  assert.match(html, /What works today/);
  assert.match(html, /href="\/docs\/external-executor"/);
  assert.doesNotMatch(html, /budget administration/);
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
  assert.match(html, /FULL SERVICE TOPOLOGY/);
  assert.doesNotMatch(html, /HIGH-LEVEL TOPOLOGY/);
  assert.match(html, /Every process and owned store, for readers who want the detail\./);
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

test("leads the architecture page with a five-step single-request walkthrough", async () => {
  const html = await readFile(new URL("../architecture/index.html", import.meta.url), "utf8");
  const walkthroughStart = html.indexOf('<section id="walkthrough"');
  const topologyStart = html.indexOf('<section id="topology"');
  const componentsStart = html.indexOf('<section id="components"');
  assert.ok(walkthroughStart > html.indexOf('<section class="hero">'), "walkthrough follows the hero");
  assert.ok(topologyStart > walkthroughStart, "walkthrough appears before the full topology");
  assert.ok(componentsStart > topologyStart, "component drills follow the full topology");

  const walkthrough = html.slice(walkthroughStart, topologyStart);
  assert.match(walkthrough, /FOLLOW ONE REQUEST/);
  assert.match(walkthrough, /<h2 id="walkthrough-title">One governed request,<br \/>five steps\.<\/h2>/);
  assert.match(walkthrough, /<ol class="request-flow" aria-label="Lifecycle of one governed request">/);
  const steps = [...walkthrough.matchAll(/<li class="flow-step (agent|hubu|gongbu)">[\s\S]*?<strong>([^<]+)<\/strong>/g)];
  assert.deepEqual(
    steps.map((match) => [match[1], match[2]]),
    [
      ["agent", "Agent asks for work"],
      ["hubu", "Check identity, policy, and budget"],
      ["hubu", "Authorize and reserve"],
      ["gongbu", "Execute with the provider"],
      ["hubu", "Settle, release, or reconcile"],
    ],
  );
  assert.match(walkthrough, /HUBU · CONTROL PLANE/);
  assert.match(walkthrough, /GONGBU · EXECUTION PLANE/);
  assert.match(walkthrough, /never a provider key/);
  assert.match(walkthrough, /which authenticates to Hubu on its behalf/);
  assert.doesNotMatch(walkthrough, /carrying only its Hubu credential/);
  assert.match(walkthrough, /run by Gongbu, the first-party executor/);
  assert.match(walkthrough, /<a href="\/docs\/external-executor">Your own executor<\/a>/);
  assert.doesNotMatch(walkthrough, /Every billable call follows|Provider keys live only in Gongbu/);

  const jumpLinks = [...html.matchAll(/<nav class="hero-meta"[\s\S]*?<\/nav>/g)][0][0];
  const targets = [...jumpLinks.matchAll(/href="#([^"]+)"/g)].map((match) => match[1]);
  assert.deepEqual(targets, ["walkthrough", "topology", "components"]);
  for (const id of targets) assert.match(html, new RegExp(`id="${id}"`));

  assert.match(
    html,
    /<a href="\/architecture\/internal\/">Engineering explorer <span class="nav-note">\(for contributors\)<\/span> ↗<\/a>/,
  );
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

test("feedback is discoverable and renders usable public intake links", async () => {
  const home = await (await render()).text();
  assert.match(home, /href="\/docs\/feedback">Send feedback/);
  const response = await render("/docs/feedback");
  assert.equal(response.status, 200);
  const html = await response.text();
  assert.match(html, /issues\/new\?template=bug.md/);
  assert.match(html, /issues\/new\?template=idea.md/);
  assert.match(html, /hubu_prepare_feedback/);
  assert.match(html, /Manual fallback/);
  assert.match(html, /explicit authorization/);
});

test("publishes canonical and share metadata for the static architecture page", async () => {
  const html = await readFile(new URL("../dist/client/architecture/index.html", import.meta.url), "utf8");
  assert.match(html, /<link rel="canonical" href="https:\/\/hubustack\.dev\/architecture\/"/);
  assert.match(html, /<meta property="og:title" content="Architecture · Hubu Docs"/);
  assert.match(html, /<meta property="og:image" content="https:\/\/hubustack\.dev\/og-architecture\.png"/);
  assert.match(html, /<meta name="twitter:card" content="summary_large_image"/);
  assert.match(html, /<link rel="icon" href="\/favicon\.svg" type="image\/svg\+xml"/);
});

test("applies baseline security headers to Worker responses and static assets", async () => {
  const expected = {
    "strict-transport-security": "max-age=31536000; includeSubDomains",
    "x-content-type-options": "nosniff",
    "referrer-policy": "strict-origin-when-cross-origin",
    "content-security-policy": "frame-ancestors 'none'",
  };
  for (const pathname of ["/", "/docs/local-stack", "/docs/nope"]) {
    const response = await render(pathname);
    for (const [name, value] of Object.entries(expected)) {
      assert.equal(response.headers.get(name), value, `${name} on ${pathname}`);
    }
  }
  // Static assets are served by Cloudflare before the Worker, so they rely on _headers.
  const rules = (await readFile(new URL("../dist/client/_headers", import.meta.url), "utf8")).toLowerCase();
  assert.match(rules, /^\/\*$/m);
  for (const [name, value] of Object.entries(expected)) {
    assert.ok(rules.includes(`${name}: ${value.toLowerCase()}`), `_headers lacks ${name}`);
  }
});

test("heading slugger matches github-slugger, including collisions with suffixed headings", async () => {
  const { createSlugger, githubSlug, headingText } = await import("../scripts/heading-ids.mjs");
  assert.equal(githubSlug("V4.4 Executor Request Identity"), "v44-executor-request-identity");
  assert.equal(githubSlug("Edit credentials.toml"), "edit-credentialstoml");
  assert.equal(githubSlug("schema_version"), "schema_version");
  const slug = createSlugger();
  assert.deepEqual(["Foo", "Foo-1", "Foo", "Foo"].map(slug), ["foo", "foo-1", "foo-2", "foo-3"]);

  const { Marked, Renderer } = await import("marked");
  const seen = [];
  const renderer = new Renderer();
  renderer.heading = function ({ tokens }) {
    seen.push(headingText(this.parser, tokens));
    return "";
  };
  new Marked({ renderer }).parse("## **Setup** with [a link](x), `code` &amp; *em*\n");
  assert.deepEqual(seen, ["Setup with a link, code & em"]);
});

test("heading ids follow GitHub's slug rules and the table of contents matches them", async () => {
  const contract = await (await render("/docs/spend-executor-contract")).text();
  // GitHub drops "." rather than hyphenating it.
  assert.match(contract, /id="v44-executor-request-identity"/);
  assert.match(contract, /href="#v44-executor-request-identity"/);
  assert.doesNotMatch(contract, /id="v4-4-executor-request-identity"/);

  const providers = await (await render("/configuration/local-stack/v1/providers-toml")).text();
  // Inline code headings use the code text, keeping underscores, with no "code-" wrapper.
  assert.match(providers, /<h3 id="schema_version">/);
  assert.doesNotMatch(providers, /id="code-/);

  const source = await readFile(new URL("../app/generated-docs.ts", import.meta.url), "utf8");
  const documents = JSON.parse(source.slice(source.indexOf("["), source.lastIndexOf("]") + 1));
  for (const document of documents) {
    const ids = new Set([...document.html.matchAll(/ id="([^"]+)"/g)].map((match) => match[1]));
    for (const heading of document.headings) {
      assert.ok(ids.has(heading.id), `${document.slug}: "On this page" entry #${heading.id} has no matching heading`);
    }
  }
});

