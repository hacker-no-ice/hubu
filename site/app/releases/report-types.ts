export type Verdict = "holds" | "holds-with-caveat" | "does-not-hold" | "not-tested";
// Kit identifiers come from site/app/releases/claims.json.
export type EpisodeId = string;
export type EpisodeResult = {
  verdict: Verdict;
  execution: "complete" | "incomplete" | "not-run";
  summary: string;
  caveat: string;
  metrics: { label: string; value: string }[];
};

export type ReleaseSupplement = { kitsCommit: string; recordedAt: string; episodes: EpisodeId[] };

export type ReleaseReport = {
  release: string;
  sourceCommit: string;
  kitsCommit: string | null;
  recordedAt: string | null;
  episodes: Record<EpisodeId, EpisodeResult>;
  // Kits added later against the same released source; never replacements.
  supplements?: ReleaseSupplement[];
};

