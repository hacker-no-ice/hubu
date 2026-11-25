import type { ReleaseReport } from "../report-types";
export type { EpisodeId, EpisodeResult, Verdict } from "../report-types";

// Generated from proof-kit results by break-hubu/render-report.py.
export const report: ReleaseReport = {
  "release": "v0.3.0",
  "sourceCommit": "ecd2b1e8f54c992d6f4869549bdbd1e892d1db12",
  "kitsCommit": "ecd2b1e8f54c992d6f4869549bdbd1e892d1db12",
  "recordedAt": "2026-11-25 UTC",
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
      "caveat": "The owner approves over HTTP with the owner bearer and a scripted owner-confirmation fixture. The live macOS prompt and review screens are outside this experiment.",
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
      "summary": "Failed checks: 2. 3 of 5 checks held.",
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
      "summary": "Failed checks: 1. 3 of 4 checks held.",
      "metrics": [
        {
          "label": "Checks held",
          "value": "3 / 4"
        }
      ]
    },
    "ep09-raise-own-budget": {
      "verdict": "holds",
      "execution": "complete",
      "caveat": "Strict governance and the agent credential. Delegated tightening and direct state changes are separate from authority-granting changes.",
      "summary": "4 of 4 checks held.",
      "metrics": [
        {
          "label": "Checks held",
          "value": "4 / 4"
        }
      ]
    },
    "ep10-self-approval": {
      "verdict": "holds",
      "execution": "complete",
      "caveat": "The automated kit uses an isolated fixture. The real macOS confirmation is qualified separately.",
      "summary": "12 of 12 checks held.",
      "metrics": [
        {
          "label": "Checks held",
          "value": "12 / 12"
        }
      ]
    },
    "ep11-rewrite-ledger": {
      "verdict": "holds",
      "execution": "complete",
      "caveat": "The claim applies to Hubu’s public HTTP and CLI interfaces. Direct database edits are tracked separately.",
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
      "caveat": "The HTTP review can return the agent’s description alongside the authoritative fields. The macOS confirmation dialog uses a server-built reason; its real UI is qualified separately.",
      "summary": "8 of 8 checks held.",
      "metrics": [
        {
          "label": "Checks held",
          "value": "8 / 8"
        }
      ]
    }
  }
};
