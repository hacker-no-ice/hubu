// Human-written, per-release context for the system card. Measured results come
// from each release's generated report.ts; this file only adds claim notes and
// marks challenges a release documents as known gaps without measuring them.
// A measured result always takes precedence over a documented gap.
export type ReleaseAssessment = {
  notes: Partial<Record<"C1" | "C2" | "C3" | "C4" | "C5" | "C6", string>>;
  documented: Record<string, "known-gap">;
};

const identityAndBearerGaps = { "C3.1": "known-gap", "C3.2": "known-gap", "C5.1": "known-gap" } as const;

export const assessments: Record<string, ReleaseAssessment> = {
  "v0.2.4": {
    notes: {
      C3: "Registered agent accounts are not bound to an authenticated runtime or thread.",
      C5: "The normal bearer can change budgets and policies.",
      C6: "Provider packs did not exist in v0.2.4; every provider was a built-in adapter.",
    },
    documented: identityAndBearerGaps,
  },
  "v0.2.5": {
    notes: {
      C3: "Registered agent accounts are not bound to an authenticated runtime or thread. Strong binding is planned for a future release.",
      C5: "The normal bearer can change budgets and policies. hubu ui holds the bearer and approval credentials in a local process, not per-user accounts.",
      C6: "Only verified packs from the repository ship in v0.2.5, and pack validation rejects packs that weaken engine-owned settlement or credential hosts.",
    },
    documented: identityAndBearerGaps,
  },
};

export function assessmentFor(release: string): ReleaseAssessment {
  return assessments[release] ?? { notes: {}, documented: {} };
}
