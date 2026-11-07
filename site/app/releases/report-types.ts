export type Verdict = "holds" | "holds-with-caveat" | "does-not-hold" | "not-tested";
// Kit identifiers come from site/app/releases/claims.json.
export type EpisodeId = string;
export type EvidenceLink = { label: string; href: string };

// These are observations from exported public evidence, never acceptance targets.
// Monetary plot values are integer millionths of USD; display formatting is not verification.
export type EvidenceVisual =
  | { kind: "consumption"; budgetMicrousd: number; points: { attempt: number; consumedMicrousd: number }[]; denied: number; source: EvidenceLink }
  | { kind: "costs"; authorizedMicrousd: number; recordedMicrousd: number; overrunMicrousd: number; reconciled: boolean; source: EvidenceLink }
  | { kind: "timeline"; events: { label: string; elapsedSeconds: number; detail: string }[]; source: EvidenceLink }
  | { kind: "burst"; allowed: number; denied: number; errors: number; reservedMicrousd: number; consumedMicrousd: number; remainingMicrousd: number; source: EvidenceLink };

export type EpisodeResult = {
  verdict: Verdict;
  execution: "complete" | "incomplete" | "not-run";
  summary: string;
  caveat: string;
  metrics: { label: string; value: string }[];
  visual: EvidenceVisual | null;
  terminal: string | null;
  evidence: EvidenceLink[];
};

export type ReleaseSupplement = { kitsCommit: string; recordedAt: string; host: string; episodes: EpisodeId[] };

export type ReleaseReport = {
  release: string;
  sourceCommit: string;
  kitsCommit: string | null;
  recordedAt: string | null;
  host: string | null;
  evidenceBundle: EvidenceLink | null;
  episodes: Record<EpisodeId, EpisodeResult>;
  // Kits added later against the same released source; never replacements.
  supplements?: ReleaseSupplement[];
};

