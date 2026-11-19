import data from "./assessments.json";

// Human-written, per-release context for the system card (assessments.json, also
// read by break-hubu/render-report.py). Measured results come from each release's
// generated report.ts; this only adds claim notes and marks challenges a release
// documents as known gaps without measuring them. A measured result always wins.
export type ReleaseAssessment = {
  notes: Partial<Record<"C1" | "C2" | "C3" | "C4" | "C5" | "C6", string>>;
  documented: Record<string, "known-gap">;
};

const assessments = data as Record<string, ReleaseAssessment>;

export function assessmentFor(release: string): ReleaseAssessment {
  return assessments[release] ?? { notes: {}, documented: {} };
}
