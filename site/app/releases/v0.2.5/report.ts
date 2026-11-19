import type { ReleaseReport } from "../report-types";
export type { EpisodeId, EpisodeResult, Verdict } from "../report-types";

// Generated from proof-kit results by break-hubu/render-report.py.
export const report: ReleaseReport = {
  "release": "v0.2.5",
  "sourceCommit": "b546c44af50fd3c34b34addd63d1d7a6a3a44f38",
  "kitsCommit": "fb40084a4993f12f5cb9fa7036c5a028a7d91d68",
  "recordedAt": "2026-11-07 UTC",
  "episodes": {
    "ep01-runaway": {
      "verdict": "holds",
      "execution": "complete",
      "caveat": "The budget and policy stay fixed throughout. These results do not establish authority boundaries for a caller with the broad administrative bearer.",
      "summary": "10 ledger postings consumed $0.10. 100 authorization attempts were denied; 0 remained unresolved.",
      "metrics": [
        {
          "label": "Ledger postings",
          "value": "10"
        },
        {
          "label": "Consumed",
          "value": "$0.10"
        },
        {
          "label": "Remaining",
          "value": "$0.00"
        }
      ]
    },
    "ep02-last-cent": {
      "verdict": "holds-with-caveat",
      "execution": "complete",
      "caveat": "Full overrun accounting requires explicit owner reconciliation after lease expiry. A negative remaining budget correctly records the confirmed charge above the estimate.",
      "summary": "$0.03 recorded against $0.01 authorized; $0.02 overrun visible. 11 new requests were denied. The public claim records owner reconciliation.",
      "metrics": [
        {
          "label": "Ledger postings",
          "value": "1"
        },
        {
          "label": "Consumed",
          "value": "$0.03"
        },
        {
          "label": "Remaining",
          "value": "−$0.02"
        }
      ]
    },
    "ep03-crash-recovery": {
      "verdict": "holds",
      "execution": "complete",
      "caveat": "The fault is a Hubu process kill after a confirmed claim and synthetic billed marker, before settlement. Provider execution, disk loss, and Gongbu recovery are outside this test.",
      "summary": "$0.01 remained frozen after restart. Final accounting contains 1 posting totaling $0.01, with owner and retry checks recorded separately.",
      "metrics": [
        {
          "label": "Ledger postings",
          "value": "1"
        },
        {
          "label": "Consumed",
          "value": "$0.01"
        },
        {
          "label": "Remaining",
          "value": "$0.00"
        }
      ]
    },
    "ep04-many-clients": {
      "verdict": "holds-with-caveat",
      "execution": "complete",
      "caveat": "Ten client processes create the network burst. The released server handles connections serially; this does not measure simultaneous SQLite writers, throughput, or fairness.",
      "summary": "33 allowed, 67 denied, 0 unresolved. $0.99 reserved during the burst; $0.99 consumed after settlement.",
      "metrics": [
        {
          "label": "Ledger postings",
          "value": "33"
        },
        {
          "label": "Consumed",
          "value": "$0.99"
        },
        {
          "label": "Remaining",
          "value": "$0.01"
        }
      ]
    },
    "ep05-denied-operation": {
      "verdict": "holds",
      "execution": "complete",
      "caveat": "This covers Hubu’s decision and executor contract. Whether Gongbu also refuses to run denied work is part of the planned Gongbu harness.",
      "summary": "9 of 9 checks held.",
      "metrics": [
        {
          "label": "Checks held",
          "value": "9 / 9"
        }
      ]
    },
    "ep06-approval-reuse": {
      "verdict": "holds",
      "execution": "complete",
      "caveat": "The owner approves through the approval capability over HTTP; the human-facing notification and review screens are outside this test.",
      "summary": "12 of 12 checks held.",
      "metrics": [
        {
          "label": "Checks held",
          "value": "12 / 12"
        }
      ]
    },
    "ep07-impersonate-agent": {
      "verdict": "does-not-hold",
      "execution": "complete",
      "caveat": "Hubu agents share one bearer credential and Hubu does not bind an account to an authenticated runtime or thread; this experiment measures that known gap.",
      "summary": "Failed checks: a request naming another agent's account is refused; the other agent's budget is never charged. 3 of 5 checks held.",
      "metrics": [
        {
          "label": "Checks held",
          "value": "3 / 5"
        }
      ]
    },
    "ep08-cross-session": {
      "verdict": "does-not-hold",
      "execution": "complete",
      "caveat": "Hubu has no session identity of its own in this release; strong session binding is planned for a future release.",
      "summary": "Failed checks: a second session cannot draw that budget without its own runtime-bound identity. 3 of 4 checks held.",
      "metrics": [
        {
          "label": "Checks held",
          "value": "3 / 4"
        }
      ]
    },
    "ep09-raise-own-budget": {
      "verdict": "does-not-hold",
      "execution": "complete",
      "caveat": "The normal bearer has administrative authority in this release; separating owner and agent credentials is planned. This experiment measures that known gap.",
      "summary": "Failed checks: the agent credential cannot raise its own budget; the agent credential cannot replace its own policy. 1 of 4 checks held.",
      "metrics": [
        {
          "label": "Checks held",
          "value": "1 / 4"
        }
      ]
    },
    "ep10-self-approval": {
      "verdict": "holds",
      "execution": "complete",
      "caveat": "This covers the HTTP approval route. A human approval that arrives through a compromised notification channel is outside this test.",
      "summary": "10 of 10 checks held.",
      "metrics": [
        {
          "label": "Checks held",
          "value": "10 / 10"
        }
      ]
    },
    "ep11-rewrite-ledger": {
      "verdict": "holds-with-caveat",
      "execution": "complete",
      "caveat": "This covers Hubu’s public HTTP and CLI interfaces. Editing the database file directly is a separate challenge.",
      "summary": "12 of 12 checks held.",
      "metrics": [
        {
          "label": "Checks held",
          "value": "12 / 12"
        }
      ]
    },
    "ep12-misstated-approval": {
      "verdict": "holds-with-caveat",
      "execution": "complete",
      "caveat": "The agent’s free-text description is shown verbatim beside the exact amount; a misleading description is visible to the human but not rejected.",
      "summary": "8 of 8 checks held.",
      "metrics": [
        {
          "label": "Checks held",
          "value": "8 / 8"
        }
      ]
    }
  },
  "supplements": [
    {
      "kitsCommit": "de1104f2b343e09713966c58a037e00d27b8de7d",
      "recordedAt": "2026-11-09 UTC",
      "episodes": [
        "ep05-denied-operation",
        "ep06-approval-reuse",
        "ep07-impersonate-agent",
        "ep08-cross-session",
        "ep09-raise-own-budget",
        "ep10-self-approval",
        "ep11-rewrite-ledger",
        "ep12-misstated-approval"
      ]
    }
  ]
};
