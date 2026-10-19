import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { readFile } from "node:fs/promises";
import test from "node:test";
import vm from "node:vm";

async function exportedString(path) {
  const source = await readFile(new URL(path, import.meta.url), "utf8");
  const match = source.match(/String\.raw`([\s\S]*)`;/);
  assert.ok(match, "module exports a literal asset");
  return match[1];
}

const beacon = await exportedString("../worker/analytics-beacon.ts");
const dashboard = await exportedString("../worker/analytics-dashboard.ts");
const dashboardScript = dashboard.match(/<script>([\s\S]*?)<\/script>/)[1];
const tick = () => new Promise((resolve) => setImmediate(resolve));

function tracker(options = {}) {
  const requests = [];
  const windowListeners = new Map();
  const documentListeners = new Map();
  const stored = new Map();
  const clock = { now: Date.UTC(2026, 9, 19, 12) };
  const location = { protocol: "https:", pathname: "/", ...options.location };
  function on(map, name, callback) {
    const listeners = map.get(name) ?? [];
    listeners.push(callback);
    map.set(name, listeners);
  }
  const window = {
    addEventListener: (name, callback) => on(windowListeners, name, callback),
    doNotTrack: options.windowDnt,
  };
  window.self = window;
  window.top = options.framed ? {} : window;
  const document = {
    visibilityState: "visible",
    prerendering: false,
    cookie: "",
    referrer: "https://example.com/story?ignored=here",
    addEventListener: (name, callback) => on(documentListeners, name, callback),
    ...options.document,
  };
  const history = Object.fromEntries(["pushState", "replaceState"].map((method) => [method, function (_state, _unused, url) {
    if (url) location.pathname = new URL(url, "https://hubustack.dev" + location.pathname).pathname;
    return "original-result";
  }]));
  const context = vm.createContext({
    window, document, location, history,
    navigator: { webdriver: false, ...options.navigator },
    crypto: { randomUUID }, TextEncoder, URL,
    Date: class extends Date { static now() { return clock.now; } },
    sessionStorage: {
      getItem(key) { if (options.storageBlocked) throw new Error("blocked"); return stored.get(key) ?? null; },
      setItem(key, value) { if (options.storageBlocked) throw new Error("blocked"); stored.set(key, value); },
    },
    fetch: (url, init) => {
      requests.push({ url, ...init, payload: JSON.parse(init.body) });
      return options.fetch ? options.fetch(url, init) : Promise.resolve(new Response(null, { status: 204 }));
    },
  });
  const fire = (map, name, event = {}) => (map.get(name) ?? []).forEach((listener) => listener(event));
  return {
    requests, document, location, history, clock, stored,
    run: () => vm.runInContext(beacon, context),
    windowEvent: (name, event) => fire(windowListeners, name, event),
    documentEvent: (name, event) => fire(documentListeners, name, event),
  };
}

test("beacon measures real route changes once, including BFCache restores", async () => {
  const browser = tracker();
  browser.run();
  browser.run();
  await tick();
  assert.equal(browser.requests.length, 1);
  assert.equal(browser.history.pushState({}, "", "/docs?token=secret#anchor"), "original-result");
  browser.history.replaceState({}, "", "/docs?other=secret#another");
  browser.windowEvent("popstate");
  browser.windowEvent("pageshow", { persisted: false });
  await tick();
  assert.deepEqual(browser.requests.map((request) => request.payload.path), ["/", "/docs"]);
  browser.windowEvent("pageshow", { persisted: true });
  await tick();
  assert.equal(browser.requests.length, 3);
  assert.equal(browser.requests[2].payload.path, "/docs");
  assert.equal(new Set(browser.requests.map((request) => request.payload.eventId)).size, 3);
  assert.equal(new Set(browser.requests.map((request) => request.payload.sessionId)).size, 1);
  assert.equal(browser.requests[0].credentials, "same-origin");
  assert.equal(browser.requests[0].keepalive, true);
  assert.equal(browser.requests[0].headers["content-type"], "application/json");
  assert.equal(browser.requests[0].payload.referrer, "https://example.com");
});

test("beacon sends only HTTP referrer origins and never referrer secrets", async () => {
  const browser = tracker({ document: { referrer: "https://person:password-secret@source.example:8443/private/path?token=query-secret#fragment-secret" } });
  browser.run();
  await tick();
  assert.equal(browser.requests[0].payload.referrer, "https://source.example:8443");
  assert.doesNotMatch(browser.requests[0].body, /password-secret|private\/path|query-secret|fragment-secret/);
  assert.equal(browser.requests[0].url, "/_telemetry/collect");
  for (const referrer of ["", "malformed referrer", "about:blank", "data:text/plain,secret"]) {
    const other = tracker({ document: { referrer } });
    other.run();
    await tick();
    assert.equal(other.requests[0].payload.referrer, "", referrer);
  }
});

test("beacon respects privacy choices and does not track frames or its dashboard", async () => {
  const choices = [
    { navigator: { globalPrivacyControl: true } },
    { navigator: { doNotTrack: "1" } },
    { windowDnt: "1" },
    { navigator: { msDoNotTrack: "1" } },
    { document: { cookie: "other=ok; hubu_analytics_exclude=1" } },
    { framed: true },
    { location: { protocol: "file:" } },
    { location: { pathname: "/_analytics" } },
    { location: { pathname: "/_analytics/report" } },
  ];
  for (const options of choices) {
    const browser = tracker(options);
    browser.run();
    await tick();
    assert.equal(browser.requests.length, 0, JSON.stringify(options));
  }
});

test("beacon waits for visible prerender activation and sends automation signals", async () => {
  const browser = tracker({ document: { visibilityState: "hidden", prerendering: true }, navigator: { webdriver: true } });
  browser.run();
  await tick();
  assert.equal(browser.requests.length, 0);
  browser.document.prerendering = false;
  browser.documentEvent("prerenderingchange");
  await tick();
  assert.equal(browser.requests.length, 0);
  browser.document.visibilityState = "visible";
  browser.documentEvent("visibilitychange");
  await tick();
  assert.equal(browser.requests.length, 1);
  assert.equal(browser.requests[0].payload.automated, true);
});

test("beacon session id expires after 30 idle minutes and tolerates blocked storage", async () => {
  for (const storageBlocked of [false, true]) {
    const browser = tracker({ storageBlocked });
    browser.run();
    await tick();
    browser.clock.now += 60 * 1000;
    browser.history.pushState({}, "", "/one");
    await tick();
    browser.clock.now += 30 * 60 * 1000;
    browser.history.pushState({}, "", "/two");
    await tick();
    const sessions = browser.requests.map((request) => request.payload.sessionId);
    assert.equal(sessions[0], sessions[1]);
    assert.notEqual(sessions[1], sessions[2]);
  }
});

test("beacon serializes its first cookie-setting request with subsequent navigation", async () => {
  let finishFirst;
  let calls = 0;
  const browser = tracker({ fetch: () => ++calls === 1 ? new Promise((resolve) => { finishFirst = resolve; }) : Promise.resolve(new Response(null, { status: 204 })) });
  browser.run();
  browser.history.pushState({}, "", "/next");
  await tick();
  assert.equal(browser.requests.length, 1);
  finishFirst(new Response(null, { status: 204 }));
  await tick();
  assert.equal(browser.requests.length, 2);
});

test("beacon bounds Unicode payloads and isolates fetch failure from navigation", async () => {
  const browser = tracker({ location: { pathname: "/" + "🦭".repeat(600) }, document: { referrer: "https://example.com/" + "🦭".repeat(600) }, fetch: () => Promise.reject(new Error("offline")) });
  assert.doesNotThrow(browser.run);
  await tick();
  assert.ok(Buffer.byteLength(browser.requests[0].body) < 2000);
  assert.ok(browser.requests[0].payload.path.length <= 512);
  assert.ok(browser.requests[0].payload.referrer.length <= 512);
  assert.doesNotThrow(() => browser.history.pushState({}, "", "/safe"));
  await tick();
  assert.equal(browser.requests.length, 2);
});

class Node {
  children = [];
  attributes = {};
  listeners = {};
  textContent = "";
  hidden = false;
  disabled = false;
  dataset = {};
  appendChild(child) { this.children.push(child); return child; }
  replaceChildren(...children) { this.children = children; }
  setAttribute(key, value) { this.attributes[key] = value; }
  addEventListener(name, callback) { this.listeners[name] = callback; }
  set innerHTML(_value) { throw new Error("Untrusted HTML insertion"); }
}

function report(uniqueVisitors = 11, enabled = true) {
  return {
    enabled, days: 7, since: "2026-10-13T00:00:00.000Z", until: "2026-10-19T12:00:00.000Z", timezone: "UTC",
    summary: { uniqueVisitors, visits: 13, pageViews: 91, newVisitors: 8, returningVisitors: 3, excludedEvents: 5 },
    daily: [{ date: "2026-10-19", uniqueVisitors, visits: 13, pageViews: 91, excludedEvents: 5 }],
    pages: [{ path: '<img src="x" onerror="alert(1)">', views: 91, visitors: uniqueVisitors }],
    referrers: [{ referrer: "t.co", visits: 7, visitors: 6 }],
    countries: [{ country: "US", visitors: uniqueVisitors }], exclusions: [{ reason: "automated_browser", count: 5 }],
  };
}

function analyticsDashboard(fetch) {
  const nodes = new Map();
  const buttons = [1, 7, 30].map((days) => { const node = new Node(); node.dataset.days = String(days); return node; });
  let cookie = "";
  const document = {
    getElementById(id) { if (!nodes.has(id)) nodes.set(id, new Node()); return nodes.get(id); },
    querySelectorAll() { return buttons; },
    createElement: () => new Node(), createElementNS: () => new Node(),
    get cookie() { return cookie; },
    set cookie(value) { cookie = value.includes("Max-Age=0;") ? "" : value.split(";")[0]; },
  };
  vm.runInNewContext(dashboardScript, { document, window: { addEventListener() {} }, Intl, AbortController, fetch });
  return { nodes, buttons, document };
}

function json(data) { return new Response(JSON.stringify(data), { headers: { "content-type": "application/json" } }); }

test("dashboard renders values safely, explains estimates, and toggles the exclusion cookie", async () => {
  const requests = [];
  const ui = analyticsDashboard(async (url, init) => { requests.push({ url, ...init }); return json(report()); });
  await tick();
  assert.equal(ui.nodes.get("uniqueVisitors").textContent, "11");
  assert.equal(ui.nodes.get("pages").children[0].children[0].textContent, '<img src="x" onerror="alert(1)">');
  assert.equal(ui.nodes.get("report").attributes["aria-busy"], "false");
  assert.equal(requests[0].url, "/_analytics/report?days=7");
  assert.equal(requests[0].cache, "no-store");
  assert.equal(requests[0].credentials, "same-origin");
  assert.match(dashboard, /Multiple devices and cleared cookies create additional identities/);
  assert.match(dashboard, /not all automated requests or bots blocked by Cloudflare/);
  ui.nodes.get("toggle-exclusion").listeners.click();
  assert.equal(ui.document.cookie, "hubu_analytics_exclude=1");
  assert.equal(ui.nodes.get("toggle-exclusion").textContent, "Include this browser");
  ui.nodes.get("toggle-exclusion").listeners.click();
  assert.equal(ui.document.cookie, "");
  assert.equal(ui.nodes.get("toggle-exclusion").textContent, "Exclude this browser");
});

test("dashboard ignores an obsolete request after the range changes", async () => {
  const pending = [];
  const ui = analyticsDashboard((url, init) => new Promise((resolve) => pending.push({ url, signal: init.signal, resolve })));
  ui.buttons[2].listeners.click();
  assert.equal(pending[0].signal.aborted, true);
  pending[1].resolve(json(report(30)));
  await tick();
  pending[0].resolve(json(report(7)));
  await tick();
  assert.equal(ui.nodes.get("uniqueVisitors").textContent, "30");
  assert.equal(ui.buttons[2].attributes["aria-pressed"], "true");
});

test("dashboard distinguishes disabled collection from an empty audience", async () => {
  const data = report(0, false);
  data.summary.pageViews = 0;
  const ui = analyticsDashboard(async () => json(data));
  await tick();
  assert.equal(ui.nodes.get("disabled").hidden, false);
  assert.equal(ui.nodes.get("empty").hidden, true);
  assert.match(ui.nodes.get("status").textContent, /Collection is disabled/);
});

test("dashboard exposes authentication and load errors without claiming zero traffic", async () => {
  const ui = analyticsDashboard(async () => new Response("unauthorized", { status: 401 }));
  await tick();
  assert.equal(ui.nodes.get("error").hidden, false);
  assert.match(ui.nodes.get("error").textContent, /session has expired/);
  assert.equal(ui.nodes.get("period").textContent, "Report unavailable");
  assert.equal(ui.nodes.get("refresh").disabled, false);
});
