export type Verdict = "holds" | "holds-with-caveat" | "does-not-hold" | "not-tested";
export type EpisodeId = "ep01-runaway" | "ep02-last-cent" | "ep03-crash-recovery" | "ep04-many-clients";
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

export type ReleaseReport = {
  release: string;
  sourceCommit: string;
  kitsCommit: string | null;
  recordedAt: string | null;
  host: string | null;
  evidenceBundle: EvidenceLink | null;
  episodes: Record<EpisodeId, EpisodeResult>;
};

