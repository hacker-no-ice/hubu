import assert from "node:assert/strict";
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
  assert.match(html, /stack start/);
  assert.doesNotMatch(html, /codex-preview|react-loading-skeleton/i);
});

test("renders canonical Markdown on a documentation route", async () => {
  const response = await render("/docs/local-stack");
  assert.equal(response.status, 200);
  const html = await response.text();
  assert.match(html, /Local stack configuration/);
  assert.match(html, /stack doctor/);
  assert.match(html, /not available on main yet/i);
  assert.match(html, /On this page/);
});
