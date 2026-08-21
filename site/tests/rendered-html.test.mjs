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
  assert.doesNotMatch(html, /not on main yet/i);
  assert.doesNotMatch(html, /codex-preview|react-loading-skeleton/i);
});

test("renders canonical Markdown on a documentation route", async () => {
  const response = await render("/docs/local-stack");
  assert.equal(response.status, 200);
  const html = await response.text();
  assert.match(html, /Local stack configuration/);
  assert.match(html, /stack doctor/);
  assert.match(html, /stack start/);
  assert.doesNotMatch(html, /not on main yet/i);
  assert.match(html, /On this page/);
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

test("uses document navigation for deployed subpage reliability", async () => {
  const sources = await Promise.all([
    readFile(new URL("../app/page.tsx", import.meta.url), "utf8"),
    readFile(new URL("../app/components/DocsShell.tsx", import.meta.url), "utf8"),
    readFile(new URL("../app/components/Search.tsx", import.meta.url), "utf8"),
  ]);
  assert.doesNotMatch(sources.join("\n"), /next\/link|<Link\b/);
});
