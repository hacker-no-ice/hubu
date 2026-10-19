/** Cloudflare Worker entry point for the vinext-starter template. */
import { handleImageOptimization, DEFAULT_DEVICE_SIZES, DEFAULT_IMAGE_SIZES } from "vinext/server/image-optimization";
import handler from "vinext/server/app-router-entry";
import { buildReport, handleCollect, pruneAnalytics } from "./analytics";
import { isAnalyticsAuthorized } from "./analytics-auth";
import { DASHBOARD_HTML } from "./analytics-dashboard";
import { BEACON_JS } from "./analytics-beacon";

declare const __HUBUSTACK_SOURCE_REVISION__: string;

const SOURCE_REVISION_PATH = "/.well-known/hubustack-revision";

// Baseline headers for every Worker response. Static assets served directly by
// Cloudflare get the same set from public/_headers. Only frame-ancestors is set
// in the CSP because the site needs inline styles/scripts and a YouTube iframe.
const SECURITY_HEADERS: Record<string, string> = {
  "strict-transport-security": "max-age=31536000; includeSubDomains",
  "x-content-type-options": "nosniff",
  "referrer-policy": "strict-origin-when-cross-origin",
  "content-security-policy": "frame-ancestors 'none'",
};

function withSourceRevision(response: Response): Response {
  const headers = new Headers(response.headers);
  headers.set("x-hubustack-revision", __HUBUSTACK_SOURCE_REVISION__);
  for (const [name, value] of Object.entries(SECURITY_HEADERS)) {
    if (name !== "content-security-policy" || !headers.has(name)) headers.set(name, value);
  }
  return new Response(response.body, {
    headers,
    status: response.status,
    statusText: response.statusText,
  });
}

// Image security config. SVG sources with .svg extension auto-skip the
// optimization endpoint on the client side (served directly, no proxy).
// To route SVGs through the optimizer (with security headers), set
// dangerouslyAllowSVG: true in next.config.js and uncomment below:
// const imageConfig: ImageConfig = { dangerouslyAllowSVG: true };

const worker = {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const url = new URL(request.url);

    if (url.pathname === "/_analytics" || url.pathname.startsWith("/_analytics/") || url.pathname.startsWith("/_telemetry/")) {
      return withSourceRevision(await analyticsResponse(request, env));
    }

    if (url.pathname === SOURCE_REVISION_PATH) {
      return withSourceRevision(new Response(__HUBUSTACK_SOURCE_REVISION__, {
        headers: {
          "cache-control": "no-store",
          "content-type": "text/plain; charset=utf-8",
        },
      }));
    }

    if (url.hostname === "hubu-docs.water-no-ice.chatgpt.site") {
      url.protocol = "https:";
      url.hostname = "hubustack.dev";
      url.port = "";
      return withSourceRevision(Response.redirect(url.href, 308));
    }

    // Superseded runbook; its content lives in the live provider operations guide.
    if (url.pathname === "/docs/operations/live-provider-testing" || url.pathname === "/docs/operations/live-provider-testing/") {
      url.pathname = "/docs/operations/live-providers";
      return withSourceRevision(Response.redirect(url.href, 308));
    }

    if (url.pathname === "/_vinext/image") {
      const allowedWidths = [...DEFAULT_DEVICE_SIZES, ...DEFAULT_IMAGE_SIZES];
      return withSourceRevision(await handleImageOptimization(request, {
        fetchAsset: (path) => env.ASSETS.fetch(new Request(new URL(path, request.url))),
        transformImage: async (body, { width, format, quality }) => {
          const result = await env.IMAGES.input(body).transform(width > 0 ? { width } : {}).output({ format: imageOutputFormat(format), quality });
          return result.response();
        },
      }, allowedWidths));
    }

    return withSourceRevision(await handler.fetch(request, env, ctx));
  },
  async scheduled(_event: ScheduledController, env: Env): Promise<void> {
    await pruneAnalytics(env);
  },
};

function imageOutputFormat(format: string): ImageOutputOptions["format"] {
  switch (format) {
    case "image/jpeg": case "image/png": case "image/gif": case "image/webp": case "image/avif": case "rgb": case "rgba":
      return format;
    default:
      throw new Error("Unsupported image format");
  }
}

async function analyticsResponse(request: Request, env: Env): Promise<Response> {
  const path = new URL(request.url).pathname;
  const headers = {
    "cache-control": "private, no-store",
    "x-robots-tag": "noindex, nofollow, noarchive",
    vary: "Cf-Access-Jwt-Assertion, Cookie",
  };
  try {
    if (path === "/_telemetry/collect") return await handleCollect(request, env);
    if (path === "/_telemetry/beacon.js") {
      if (request.method !== "GET") return new Response(null, { status: 405, headers: { ...headers, allow: "GET" } });
      return new Response(BEACON_JS, { headers: { ...headers, "content-type": "application/javascript; charset=utf-8" } });
    }
    if (!["/_analytics", "/_analytics/", "/_analytics/report"].includes(path)) {
      return new Response("Not found", { status: 404, headers });
    }
    if (!(await isAnalyticsAuthorized(request, env))) {
      return new Response("Sign in through the site's Cloudflare Access application to view analytics.", { status: 403, headers });
    }
    if (request.method !== "GET") return new Response(null, { status: 405, headers: { ...headers, allow: "GET" } });
    if (path === "/_analytics/report") {
      const days = new URL(request.url).searchParams.get("days") ?? "7";
      if (days !== "1" && days !== "7" && days !== "30") {
        return Response.json({ error: "Choose 1, 7, or 30 days." }, { status: 400, headers });
      }
      return Response.json(await buildReport(env, Number(days) as 1 | 7 | 30), { headers });
    }
    const nonce = crypto.randomUUID();
    const html = DASHBOARD_HTML.replace("<style>", `<style nonce="${nonce}">`).replace("<script>", `<script nonce="${nonce}">`);
    return new Response(html, {
      headers: {
        ...headers,
        "content-type": "text/html; charset=utf-8",
        "content-security-policy": `default-src 'none'; script-src 'nonce-${nonce}'; style-src 'nonce-${nonce}'; connect-src 'self'; img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`,
      },
    });
  } catch {
    // Never let analytics failures interrupt public page delivery or expose SQL,
    // identifiers, request bodies, or authentication tokens through error logs.
    console.error(JSON.stringify({ event: "analytics_unavailable", path }));
    return Response.json({ error: "Analytics is temporarily unavailable." }, { status: 503, headers });
  }
}

export default worker;
