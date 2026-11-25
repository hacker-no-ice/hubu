import data from "./assessments.json";
import catalog from "./claims.json";

type Claim = (typeof catalog.claims)[number];
export type Challenge = { id: string; claim: string; title: string; status: string; kit?: string; reason?: string };
type Kit = { number: string; title: string; question: string; statement: string; setup: string; steps: string[]; verify: string; boundary: string; condition?: string };

type Catalog = {
  claims: Claim[];
  challenges: Challenge[];
  kits: Record<string, Kit>;
};

type CatalogOverrides = {
  excludeClaims?: string[];
  excludeChallenges?: string[];
  claims?: Record<string, Partial<Omit<Claim, "id">>>;
  challenges?: Record<string, Partial<Omit<Challenge, "id" | "claim">>>;
  kits?: Record<string, Partial<Kit>>;
};

// Per-release claim scope, notes and known gaps, shared with the report renderer.
// Measured results come from each release's generated report.ts.
export type ReleaseAssessment = {
  notes: Partial<Record<string, string>>;
  documented: Record<string, "known-gap">;
  catalog?: CatalogOverrides;
};

const assessments = data as Record<string, Partial<ReleaseAssessment>>;

export function assessmentFor(release: string): ReleaseAssessment {
  const assessment = assessments[release];
  return { notes: assessment?.notes ?? {}, documented: assessment?.documented ?? {}, catalog: assessment?.catalog };
}

export function catalogFor(release: string): Catalog {
  const overrides = assessmentFor(release).catalog;
  if (!overrides) return catalog;

  const claims = catalog.claims.filter(({ id }) => !overrides.excludeClaims?.includes(id))
    .map((claim) => ({ ...claim, ...overrides.claims?.[claim.id] }));
  const challenges = catalog.challenges.filter(({ id, claim }) =>
    !overrides.excludeClaims?.includes(claim) && !overrides.excludeChallenges?.includes(id))
    .map((challenge) => ({ ...challenge, ...overrides.challenges?.[challenge.id] }));
  const retainedKits = new Set(challenges.flatMap((challenge) => "kit" in challenge && challenge.kit ? [challenge.kit] : []));
  const kits = Object.fromEntries(Object.entries(catalog.kits)
    .filter(([id]) => retainedKits.has(id))
    .map(([id, kit]) => [id, { ...kit, ...overrides.kits?.[id] }]));
  return { claims, challenges, kits };
}
