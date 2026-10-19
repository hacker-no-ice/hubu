import { createRemoteJWKSet, jwtVerify, type JWTVerifyGetKey } from "jose";

export type AnalyticsAccessEnv = Pick<Env, "ANALYTICS_ACCESS_TEAM_DOMAIN" | "ANALYTICS_ACCESS_AUD">;

// Only signing-key resolvers are shared between requests. Keep the cache bounded
// even if multiple environments happen to share a Worker isolate.
const signingKeys = new Map<string, ReturnType<typeof createRemoteJWKSet>>();
const MAX_CACHED_TEAMS = 4;
const TEAM_DOMAIN = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.cloudflareaccess\.com$/;

function accessConfig(env: AnalyticsAccessEnv): { issuer: string; audience: string } | null {
  const team = env.ANALYTICS_ACCESS_TEAM_DOMAIN;
  const audience = env.ANALYTICS_ACCESS_AUD;
  // A bare Cloudflare team hostname prevents configuration from redirecting JWKS
  // retrieval to arbitrary hosts, paths, ports, or embedded credentials.
  if (typeof team !== "string" || !TEAM_DOMAIN.test(team)) return null;
  if (typeof audience !== "string" || !audience || audience.trim() !== audience || audience.length > 512) return null;
  return { issuer: `https://${team}`, audience };
}

function keysForIssuer(issuer: string): ReturnType<typeof createRemoteJWKSet> {
  const cached = signingKeys.get(issuer);
  if (cached) return cached;
  const keys = createRemoteJWKSet(new URL(`${issuer}/cdn-cgi/access/certs`), {
    timeoutDuration: 3_000,
    cooldownDuration: 30_000,
    cacheMaxAge: 600_000,
  });
  if (signingKeys.size >= MAX_CACHED_TEAMS) signingKeys.delete(signingKeys.keys().next().value!);
  signingKeys.set(issuer, keys);
  return keys;
}

/** Verify an Access application token; the optional key resolver supports offline tests. */
export async function verifyAnalyticsToken(
  token: string,
  env: AnalyticsAccessEnv,
  keyResolver?: JWTVerifyGetKey,
): Promise<boolean> {
  const config = accessConfig(env);
  if (!config || typeof token !== "string" || !token || token.length > 16_384) return false;

  try {
    const { payload } = await jwtVerify(token, keyResolver ?? keysForIssuer(config.issuer), {
      issuer: config.issuer,
      audience: config.audience,
      algorithms: ["RS256"],
      requiredClaims: ["exp", "iat", "sub"],
    });
    // The dashboard is for signed-in people. Global session tokens and service
    // tokens (which have an empty subject) do not authorize report access.
    return payload.type === "app"
      && typeof payload.sub === "string" && payload.sub.trim().length > 0
      && typeof payload.iat === "number" && payload.iat <= Math.floor(Date.now() / 1_000)
      && typeof payload.exp === "number" && payload.exp > payload.iat;
  } catch {
    // Invalid tokens, unavailable keys, and JWKS network failures all deny access.
    return false;
  }
}

/** Never treat an email header or the mere presence of an assertion as authorization. */
export async function isAnalyticsAuthorized(request: Request, env: AnalyticsAccessEnv): Promise<boolean> {
  return verifyAnalyticsToken(request.headers.get("cf-access-jwt-assertion") ?? "", env);
}
