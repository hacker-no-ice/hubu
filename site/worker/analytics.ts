const VISITOR_COOKIE = "__Host-hubu_visitor";
const EXCLUDE_COOKIE = "hubu_analytics_exclude";
const MAX_BODY_BYTES = 2048;
const DAY_MS = 86_400_000;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const CRAWLER = /bot\b|crawler|spider|slurp|bingpreview|headless|lighthouse|pagespeed|phantomjs|selenium|playwright|puppeteer|curl\/|wget\/|python-requests|python-urllib|go-http-client|facebookexternalhit|whatsapp|telegrambot|discordbot|pinterest|preview/i;
const NON_PAGE_EXTENSION = /\.(?:avif|bmp|css|csv|eot|gif|ico|jpe?g|js|json|map|mjs|mov|mp3|mp4|ogg|otf|pdf|png|rss|svg|tar|txt|wasm|webm|webp|woff2?|xml|zip)$/i;

interface Beacon {
  eventId: string;
  sessionId: string;
  path: string;
  referrer: string;
  automated: boolean;
}

interface Summary {
  uniqueVisitors: number;
  visits: number;
  pageViews: number;
  newVisitors: number;
  returningVisitors: number;
  excludedEvents: number;
}

export interface AnalyticsReport {
  enabled: boolean;
  days: 1 | 7 | 30;
  since: string;
  until: string;
  timezone: "UTC";
  summary: Summary;
  daily: { date: string; uniqueVisitors: number; visits: number; pageViews: number; excludedEvents: number }[];
  pages: { path: string; views: number; visitors: number }[];
  referrers: { referrer: string; visits: number; visitors: number }[];
  countries: { country: string; visitors: number }[];
  exclusions: { reason: string; count: number }[];
}

function response(status = 204, headers: HeadersInit = {}): Response {
  return new Response(null, { status, headers: { "cache-control": "no-store", ...headers } });
}

function cookieValue(request: Request, name: string): string | undefined {
  return request.headers.get("cookie")?.split(";").map((part) => part.trim())
    .find((part) => part.startsWith(`${name}=`))?.slice(name.length + 1);
}

function canonicalOrigin(env: Env): string | null {
  try {
    const origin = new URL(env.ANALYTICS_ORIGIN || "https://hubustack.dev");
    // Misconfiguration must disable collection rather than widen accepted origins.
    const allowedProtocol = origin.protocol === "https:" || (origin.protocol === "http:" && origin.hostname === "localhost");
    return allowedProtocol && origin.href === `${origin.origin}/` ? origin.origin : null;
  } catch {
    return null;
  }
}

async function readBeacon(request: Request): Promise<Beacon | number> {
  const contentLength = request.headers.get("content-length");
  if (contentLength && (!/^\d+$/.test(contentLength) || Number(contentLength) > MAX_BODY_BYTES)) return 413;
  if (!request.body) return 400;
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  try {
    while (true) {
      const chunk = await reader.read();
      if (chunk.done) break;
      length += chunk.value.byteLength;
      if (length > MAX_BODY_BYTES) {
        await reader.cancel();
        return 413;
      }
      chunks.push(chunk.value);
    }
    const bytes = new Uint8Array(length);
    let offset = 0;
    for (const chunk of chunks) {
      bytes.set(chunk, offset);
      offset += chunk.byteLength;
    }
    const value: unknown = JSON.parse(new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(bytes));
    if (typeof value !== "object" || value === null || Array.isArray(value)) return 400;
    const beacon = value as Record<string, unknown>;
    if (Object.keys(beacon).length !== 5 || typeof beacon.eventId !== "string" || !UUID.test(beacon.eventId)
      || typeof beacon.sessionId !== "string" || !UUID.test(beacon.sessionId)
      || typeof beacon.path !== "string" || beacon.path.length > 512
      || typeof beacon.referrer !== "string" || beacon.referrer.length > 1024
      || typeof beacon.automated !== "boolean") return 400;
    return {
      eventId: beacon.eventId.toLowerCase(),
      sessionId: beacon.sessionId.toLowerCase(),
      path: beacon.path,
      referrer: beacon.referrer,
      automated: beacon.automated,
    };
  } catch {
    return 400;
  } finally {
    reader.releaseLock();
  }
}

function invalidPathCharacters(value: string, includeSpace = false): boolean {
  return Array.from(value).some((character) => {
    const code = character.charCodeAt(0);
    return character === "\\" || code <= (includeSpace ? 0x20 : 0x1f) || code === 0x7f;
  });
}

function pagePath(value: string, origin: string): string | null {
  if (!value.startsWith("/") || value.startsWith("//") || invalidPathCharacters(value, true)) return null;
  try {
    const url = new URL(value, origin);
    if (url.origin !== origin) return null;
    // Decode for exclusions too, so encoded API/asset paths cannot bypass them.
    const decoded = decodeURIComponent(url.pathname);
    if (invalidPathCharacters(decoded)) return null;
    if (/^\/(?:_analytics|_telemetry|_next|_vinext|cdn-cgi)(?:\/|$)/i.test(decoded)
      || NON_PAGE_EXTENSION.test(decoded) || /(?:^|\/)\./.test(decoded)) return null;
    return url.pathname.replace(/\/+$/, "") || "/";
  } catch {
    return null;
  }
}

function referrerHost(value: string, origin: string): string {
  try {
    const url = new URL(value);
    if (!["http:", "https:"].includes(url.protocol) || url.origin === origin) return "";
    return url.hostname.toLowerCase();
  } catch {
    return "";
  }
}

function exclusionReason(request: Request, automated: boolean): string | null {
  if (automated) return "browser_automation";
  const bot = request.cf?.botManagement;
  if (bot && typeof bot === "object") {
    if ("verifiedBot" in bot && bot.verifiedBot === true) return "verified_bot";
    const score = "score" in bot ? bot.score : undefined;
    if (typeof score === "number" && score > 0 && score < 30) return "low_bot_score";
  }
  if (CRAWLER.test(request.headers.get("user-agent") || "")) return "crawler_user_agent";
  return null;
}

async function visitorHash(value: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value));
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

/** Browser-only collection. No IP address or user-agent string reaches D1. */
export async function handleCollect(request: Request, env: Env): Promise<Response> {
  if (request.method !== "POST") return response(405, { allow: "POST" });
  const origin = canonicalOrigin(env);
  if (!origin || new URL(request.url).origin !== origin || request.headers.get("origin") !== origin
    || ![null, "same-origin"].includes(request.headers.get("sec-fetch-site"))) return response(403);
  if (env.ANALYTICS_ENABLED !== "true" || !env.ANALYTICS_DB || request.headers.get("dnt") === "1" || request.headers.get("sec-gpc") === "1"
    || cookieValue(request, EXCLUDE_COOKIE) === "1") return response();
  if (env.ANALYTICS_RATE_LIMITER) {
    const key = await visitorHash(request.headers.get("cf-connecting-ip") || "unknown");
    if (!(await env.ANALYTICS_RATE_LIMITER.limit({ key })).success) return response(429, { "retry-after": "60" });
  }
  if (request.headers.get("content-type")?.split(";")[0].trim().toLowerCase() !== "application/json") return response(415);
  const beacon = await readBeacon(request);
  if (typeof beacon === "number") return response(beacon);
  const path = pagePath(beacon.path, origin);
  if (!path) return response(400);
  const now = Date.now();
  const day = new Date(now).toISOString().slice(0, 10);
  const reason = exclusionReason(request, beacon.automated);
  if (reason) {
    await env.ANALYTICS_DB.prepare(`INSERT INTO analytics_exclusions (day, reason, count) VALUES (?, ?, 1)
      ON CONFLICT(day, reason) DO UPDATE SET count = count + 1`).bind(day, reason).run();
    return response();
  }

  const suppliedVisitor = cookieValue(request, VISITOR_COOKIE);
  const visitor = suppliedVisitor && UUID.test(suppliedVisitor) ? suppliedVisitor.toLowerCase() : crypto.randomUUID();
  const hash = await visitorHash(visitor);
  const country = typeof request.cf?.country === "string" && /^[A-Z]{2}$/.test(request.cf.country) ? request.cf.country : "Unknown";
  // D1 batch is transactional. Duplicate event IDs cannot add a visitor or view,
  // including a retry whose first response (and visitor cookie) was lost.
  const results = await env.ANALYTICS_DB.batch([
    env.ANALYTICS_DB.prepare(`INSERT OR IGNORE INTO analytics_visitors (visitor_hash, first_seen, last_seen)
      SELECT ?, ?, ? WHERE NOT EXISTS (SELECT 1 FROM analytics_events WHERE event_id = ?)`)
      .bind(hash, now, now, beacon.eventId),
    env.ANALYTICS_DB.prepare(`INSERT OR IGNORE INTO analytics_events
      (event_id, visitor_hash, session_id, recorded_at, day, path, referrer, country)
      VALUES (?, ?, ?, ?, ?, ?, ?, ?)`)
      .bind(beacon.eventId, hash, beacon.sessionId, now, day, path, referrerHost(beacon.referrer, origin), country),
    env.ANALYTICS_DB.prepare(`UPDATE analytics_visitors SET last_seen = MAX(last_seen, ?)
      WHERE visitor_hash = ? AND EXISTS (SELECT 1 FROM analytics_events WHERE event_id = ? AND visitor_hash = ? AND recorded_at = ?)`)
      .bind(now, hash, beacon.eventId, hash, now),
  ]);
  return response(204, results[1].meta.changes > 0 && (!suppliedVisitor || !UUID.test(suppliedVisitor)) ? {
    "set-cookie": `${VISITOR_COOKIE}=${visitor}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=31536000`,
  } : {});
}

/** Exact period distincts. Daily counts must never be summed for period uniques. */
export async function buildReport(env: Env, days: 1 | 7 | 30): Promise<AnalyticsReport> {
  const now = Date.now();
  const since = Math.floor(now / DAY_MS) * DAY_MS - (days - 1) * DAY_MS;
  const report: AnalyticsReport = {
    enabled: env.ANALYTICS_ENABLED === "true" && Boolean(env.ANALYTICS_DB), days, since: new Date(since).toISOString(), until: new Date(now).toISOString(), timezone: "UTC",
    summary: { uniqueVisitors: 0, visits: 0, pageViews: 0, newVisitors: 0, returningVisitors: 0, excludedEvents: 0 },
    daily: Array.from({ length: days }, (_, index) => ({ date: new Date(since + index * DAY_MS).toISOString().slice(0, 10), uniqueVisitors: 0, visits: 0, pageViews: 0, excludedEvents: 0 })),
    pages: [], referrers: [], countries: [], exclusions: [],
  };
  if (!report.enabled || !env.ANALYTICS_DB) return report;
  const startDay = report.since.slice(0, 10);
  const endDay = report.until.slice(0, 10);
  const rows = await env.ANALYTICS_DB.batch([
    env.ANALYTICS_DB.prepare(`SELECT COUNT(DISTINCT e.visitor_hash) AS uniqueVisitors,
      COUNT(DISTINCT e.visitor_hash || ':' || e.session_id) AS visits, COUNT(*) AS pageViews,
      COUNT(DISTINCT CASE WHEN v.first_seen >= ? THEN e.visitor_hash END) AS newVisitors,
      COUNT(DISTINCT CASE WHEN v.first_seen < ? THEN e.visitor_hash END) AS returningVisitors
      FROM analytics_events e JOIN analytics_visitors v ON v.visitor_hash = e.visitor_hash
      WHERE e.recorded_at >= ? AND e.recorded_at <= ?`).bind(since, since, since, now),
    env.ANALYTICS_DB.prepare(`SELECT day AS date, COUNT(DISTINCT visitor_hash) AS uniqueVisitors,
      COUNT(DISTINCT visitor_hash || ':' || session_id) AS visits, COUNT(*) AS pageViews
      FROM analytics_events WHERE recorded_at >= ? AND recorded_at <= ? GROUP BY day ORDER BY day`).bind(since, now),
    env.ANALYTICS_DB.prepare(`SELECT path, COUNT(*) AS views, COUNT(DISTINCT visitor_hash) AS visitors
      FROM analytics_events WHERE recorded_at >= ? AND recorded_at <= ? GROUP BY path ORDER BY views DESC, path LIMIT 10`).bind(since, now),
    env.ANALYTICS_DB.prepare(`WITH period_sessions AS (
        SELECT visitor_hash, session_id FROM analytics_events WHERE recorded_at >= ? AND recorded_at <= ? GROUP BY visitor_hash, session_id
      ), sources AS (
        SELECT p.visitor_hash, (SELECT referrer FROM analytics_events e
          WHERE e.visitor_hash = p.visitor_hash AND e.session_id = p.session_id ORDER BY recorded_at, rowid LIMIT 1) AS referrer
        FROM period_sessions p
      ) SELECT CASE WHEN referrer = '' THEN '(direct)' ELSE referrer END AS referrer,
      COUNT(*) AS visits, COUNT(DISTINCT visitor_hash) AS visitors
      FROM sources GROUP BY referrer ORDER BY visits DESC, referrer LIMIT 10`).bind(since, now),
    env.ANALYTICS_DB.prepare(`SELECT country, COUNT(DISTINCT visitor_hash) AS visitors
      FROM analytics_events WHERE recorded_at >= ? AND recorded_at <= ? GROUP BY country ORDER BY visitors DESC, country LIMIT 10`).bind(since, now),
    env.ANALYTICS_DB.prepare(`SELECT reason, SUM(count) AS count FROM analytics_exclusions
      WHERE day >= ? AND day <= ? GROUP BY reason ORDER BY count DESC, reason`).bind(startDay, endDay),
    env.ANALYTICS_DB.prepare(`SELECT day AS date, SUM(count) AS excludedEvents FROM analytics_exclusions
      WHERE day >= ? AND day <= ? GROUP BY day`).bind(startDay, endDay),
  ]);
  Object.assign(report.summary, rows[0].results[0]);
  const daily = rows[1].results as Omit<AnalyticsReport["daily"][number], "excludedEvents">[];
  const excludedDaily = rows[6].results as { date: string; excludedEvents: number }[];
  for (const day of report.daily) {
    Object.assign(day, daily.find((row) => row.date === day.date));
    day.excludedEvents = excludedDaily.find((row) => row.date === day.date)?.excludedEvents ?? 0;
  }
  report.pages = rows[2].results as AnalyticsReport["pages"];
  report.referrers = rows[3].results as AnalyticsReport["referrers"];
  report.countries = rows[4].results as AnalyticsReport["countries"];
  report.exclusions = rows[5].results as AnalyticsReport["exclusions"];
  report.summary.excludedEvents = report.exclusions.reduce((sum, row) => sum + row.count, 0);
  return report;
}

/** Inactive identities expire too; a visitor returning after 90 days is new. */
export async function pruneAnalytics(env: Env): Promise<void> {
  if (!env.ANALYTICS_DB) return;
  const cutoff = Date.now() - 90 * DAY_MS;
  await env.ANALYTICS_DB.batch([
    env.ANALYTICS_DB.prepare("DELETE FROM analytics_events WHERE recorded_at < ?").bind(cutoff),
    env.ANALYTICS_DB.prepare("DELETE FROM analytics_visitors WHERE last_seen < ?").bind(cutoff),
    env.ANALYTICS_DB.prepare("DELETE FROM analytics_exclusions WHERE day < ?").bind(new Date(cutoff).toISOString().slice(0, 10)),
  ]);
}
