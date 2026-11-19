import type { ReleaseReport } from "../report-types";
export type { EpisodeId, EpisodeResult, Verdict } from "../report-types";

// Generated from proof-kit results by break-hubu/render-report.py.
export const report: ReleaseReport = {
  "release": "v0.2.4",
  "sourceCommit": "5344ab565171e1aa3d3ec628641cdbfce18372af",
  "kitsCommit": "f280b3dc9488040ff9e6ea9b7b42e3447f7b1021",
  "recordedAt": "2026-10-22 UTC",
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
    }
  }
};
