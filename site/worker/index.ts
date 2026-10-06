/** Cloudflare Worker entry point for the vinext-starter template. */
import { handleImageOptimization, DEFAULT_DEVICE_SIZES, DEFAULT_IMAGE_SIZES } from "vinext/server/image-optimization";
import handler from "vinext/server/app-router-entry";

declare const __HUBUSTACK_SOURCE_REVISION__: string;

const SOURCE_REVISION_PATH = "/.well-known/hubustack-revision";

interface Env {
  ASSETS: Fetcher;
  IMAGES: {
    input(stream: ReadableStream): {
      transform(options: Record<string, unknown>): {
        output(options: { format: string; quality: number }): Promise<{ response(): Response }>;
      };
    };
  };
}

interface ExecutionContext {
  waitUntil(promise: Promise<unknown>): void;
  passThroughOnException(): void;
}

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
  for (const [name, value] of Object.entries(SECURITY_HEADERS)) headers.set(name, value);
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
      return withSourceRevision(Response.redirect(url, 308));
    }

    if (url.pathname === "/_vinext/image") {
      const allowedWidths = [...DEFAULT_DEVICE_SIZES, ...DEFAULT_IMAGE_SIZES];
      return withSourceRevision(await handleImageOptimization(request, {
        fetchAsset: (path) => env.ASSETS.fetch(new Request(new URL(path, request.url))),
        transformImage: async (body, { width, format, quality }) => {
          const result = await env.IMAGES.input(body).transform(width > 0 ? { width } : {}).output({ format, quality });
          return result.response();
        },
      }, allowedWidths));
    }

    return withSourceRevision(await handler.fetch(request, env, ctx));
  },
};

export default worker;
