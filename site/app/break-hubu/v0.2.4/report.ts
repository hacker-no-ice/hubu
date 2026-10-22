import type { EpisodeResult, ReleaseReport } from "./types";
export type { EpisodeId, EpisodeResult, EvidenceVisual, Verdict } from "./types";

const pending = (caveat: string): EpisodeResult => ({
  verdict: "not-tested", execution: "not-run",
  summary: "No captured run is attached yet. The workload below describes the experiment, not a measured result.",
  caveat, metrics: [], visual: null, terminal: null, evidence: [],
});

export const report: ReleaseReport = {
  release: "v0.2.4",
  sourceCommit: "5344ab565171e1aa3d3ec628641cdbfce18372af",
  kitsCommit: null,
  recordedAt: null,
  host: null,
  evidenceBundle: null,
  episodes: {
    "ep01-runaway": pending("The budget and policy stay fixed throughout. This does not establish authority boundaries for a caller with the broad administrative bearer."),
    "ep02-last-cent": pending("Full overrun accounting requires explicit owner reconciliation. A negative remaining budget can correctly record a confirmed charge above the estimate."),
    "ep03-crash-recovery": pending("The fault is a Hubu process kill after a confirmed claim and synthetic billed marker, before settlement. Provider execution, disk loss, and Gongbu recovery are outside this test."),
    "ep04-many-clients": pending("Ten client processes create the network burst. The released server handles connections serially; this does not measure simultaneous SQLite writers, throughput, or fairness."),
  },
};
