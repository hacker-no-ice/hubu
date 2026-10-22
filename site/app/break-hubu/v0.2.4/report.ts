import type { ReleaseReport } from "./types";
export type { EpisodeId, EpisodeResult, EvidenceVisual, Verdict } from "./types";

// Generated from captured public evidence by break-hubu/render-report.py.
export const report: ReleaseReport = {
  "release": "v0.2.4",
  "sourceCommit": "5344ab565171e1aa3d3ec628641cdbfce18372af",
  "kitsCommit": "f280b3dc9488040ff9e6ea9b7b42e3447f7b1021",
  "recordedAt": "2026-10-22 UTC",
  "host": "macOS-26.4-arm64-arm-64bit",
  "evidenceBundle": {
    "label": "Download all evidence (ZIP)",
    "href": "/break-hubu/evidence/v0.2.4/evidence.zip"
  },
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
      ],
      "visual": {
        "kind": "consumption",
        "budgetMicrousd": 100000,
        "points": [
          {
            "attempt": 0,
            "consumedMicrousd": 0
          },
          {
            "attempt": 1,
            "consumedMicrousd": 10000
          },
          {
            "attempt": 2,
            "consumedMicrousd": 20000
          },
          {
            "attempt": 3,
            "consumedMicrousd": 30000
          },
          {
            "attempt": 4,
            "consumedMicrousd": 40000
          },
          {
            "attempt": 5,
            "consumedMicrousd": 50000
          },
          {
            "attempt": 6,
            "consumedMicrousd": 60000
          },
          {
            "attempt": 7,
            "consumedMicrousd": 70000
          },
          {
            "attempt": 8,
            "consumedMicrousd": 80000
          },
          {
            "attempt": 9,
            "consumedMicrousd": 90000
          },
          {
            "attempt": 10,
            "consumedMicrousd": 100000
          },
          {
            "attempt": 110,
            "consumedMicrousd": 100000
          }
        ],
        "denied": 100,
        "source": {
          "label": "Captured request trace",
          "href": "/break-hubu/evidence/v0.2.4/ep01-runaway/requests.jsonl"
        }
      },
      "terminal": "[2026-10-22T02:03:58.652626+00:00] $ /private/tmp/break-hubu-public-cache/target/release/hubu --url http://127.0.0.1:51578 budget list\n[2026-10-22T02:03:58.658507+00:00]   budget_id: bgt_1cx0nh5ht4eq  agent_id: agt_gg8a347bmse7  status: exhausted\n    limit: $0.10  consumed: $0.10  frozen: $0.00  remaining: $0.00\n    current_version_id: bgv_h6b37ft83dwj  current_revision: 1\n    period: 2026-10-21T19:03:57.947645-07:00 -> 2998-12-31T16:00:00-08:00\nCLI exit=0\n",
      "evidence": [
        {
          "label": "Full transcript",
          "href": "/break-hubu/evidence/v0.2.4/ep01-runaway/transcript.txt"
        },
        {
          "label": "Request trace",
          "href": "/break-hubu/evidence/v0.2.4/ep01-runaway/requests.jsonl"
        },
        {
          "label": "Verification result",
          "href": "/break-hubu/evidence/v0.2.4/ep01-runaway/result.json"
        },
        {
          "label": "Final ledger (page 1)",
          "href": "/break-hubu/evidence/v0.2.4/ep01-runaway/snapshots/final/ledger-001.json"
        },
        {
          "label": "Final budget",
          "href": "/break-hubu/evidence/v0.2.4/ep01-runaway/snapshots/final/budgets.json"
        },
        {
          "label": "Run provenance",
          "href": "/break-hubu/evidence/v0.2.4/ep01-runaway/run.json"
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
      ],
      "visual": {
        "kind": "costs",
        "authorizedMicrousd": 10000,
        "recordedMicrousd": 30000,
        "overrunMicrousd": 20000,
        "reconciled": true,
        "source": {
          "label": "Public authorization and receipt",
          "href": "/break-hubu/evidence/v0.2.4/ep02-last-cent/snapshots/final/authorizations-001.json"
        }
      },
      "terminal": "[2026-10-22T02:04:30.730061+00:00] $ /private/tmp/break-hubu-public-cache/target/release/hubu --url http://127.0.0.1:51747 budget list\n[2026-10-22T02:04:30.736063+00:00]   budget_id: bgt_2ds8zy84h7de  agent_id: agt_fxr1hptmbnh4  status: exhausted\n    limit: $0.01  consumed: $0.03  frozen: $0.00  remaining: -$0.02\n    current_version_id: bgv_m31n5kjgzb7d  current_revision: 1\n    period: 2026-10-21T19:04:00.418797-07:00 -> 2998-12-31T16:00:00-08:00\nCLI exit=0\n",
      "evidence": [
        {
          "label": "Full transcript",
          "href": "/break-hubu/evidence/v0.2.4/ep02-last-cent/transcript.txt"
        },
        {
          "label": "Request trace",
          "href": "/break-hubu/evidence/v0.2.4/ep02-last-cent/requests.jsonl"
        },
        {
          "label": "Verification result",
          "href": "/break-hubu/evidence/v0.2.4/ep02-last-cent/result.json"
        },
        {
          "label": "Final ledger (page 1)",
          "href": "/break-hubu/evidence/v0.2.4/ep02-last-cent/snapshots/final/ledger-001.json"
        },
        {
          "label": "Final budget",
          "href": "/break-hubu/evidence/v0.2.4/ep02-last-cent/snapshots/final/budgets.json"
        },
        {
          "label": "Run provenance",
          "href": "/break-hubu/evidence/v0.2.4/ep02-last-cent/run.json"
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
      ],
      "visual": {
        "kind": "timeline",
        "events": [
          {
            "label": "Claim confirmed",
            "elapsedSeconds": 0.0,
            "detail": "Public claim state before the crash."
          },
          {
            "label": "Fixture saved",
            "elapsedSeconds": 0.001391,
            "detail": "Durable synthetic receipt; no provider call."
          },
          {
            "label": "Fixture saved",
            "elapsedSeconds": 0.001568,
            "detail": "Durable synthetic receipt; no provider call."
          },
          {
            "label": "Hubu process killed",
            "elapsedSeconds": 0.036131,
            "detail": "SIGKILL; process exit -9."
          },
          {
            "label": "Hubu restarted",
            "elapsedSeconds": 0.13916,
            "detail": "Same released binary and database."
          },
          {
            "label": "Public state after restart",
            "elapsedSeconds": 0.14086,
            "detail": "Frozen $0.01; consumed $0.00."
          },
          {
            "label": "Public state after lease expiry",
            "elapsedSeconds": 30.01135,
            "detail": "Frozen $0.01; consumed $0.00."
          },
          {
            "label": "Owner reconciliation",
            "elapsedSeconds": 30.093746,
            "detail": "CLI exit 0."
          },
          {
            "label": "Identical owner retry",
            "elapsedSeconds": 30.123958,
            "detail": "CLI exit 0."
          },
          {
            "label": "Hubu restarted",
            "elapsedSeconds": 30.231957,
            "detail": "Same released binary and database."
          },
          {
            "label": "Final public state",
            "elapsedSeconds": 30.233669,
            "detail": "Frozen $0.00; consumed $0.01."
          }
        ],
        "source": {
          "label": "Process events and public state exports (ZIP)",
          "href": "/break-hubu/evidence/v0.2.4/evidence.zip"
        }
      },
      "terminal": "[2026-10-22T02:05:02.181048+00:00] $ /private/tmp/break-hubu-public-cache/target/release/hubu --url http://127.0.0.1:51885 budget list\n[2026-10-22T02:05:02.189259+00:00]   budget_id: bgt_1hcfqkpj5s5h  agent_id: agt_8qkzvcn6gb7y  status: exhausted\n    limit: $0.01  consumed: $0.01  frozen: $0.00  remaining: $0.00\n    current_version_id: bgv_91nd8hk9qv5f  current_revision: 1\n    period: 2026-10-21T19:04:31.749634-07:00 -> 2998-12-31T16:00:00-08:00\nCLI exit=0\n",
      "evidence": [
        {
          "label": "Full transcript",
          "href": "/break-hubu/evidence/v0.2.4/ep03-crash-recovery/transcript.txt"
        },
        {
          "label": "Request trace",
          "href": "/break-hubu/evidence/v0.2.4/ep03-crash-recovery/requests.jsonl"
        },
        {
          "label": "Verification result",
          "href": "/break-hubu/evidence/v0.2.4/ep03-crash-recovery/result.json"
        },
        {
          "label": "Final ledger (page 1)",
          "href": "/break-hubu/evidence/v0.2.4/ep03-crash-recovery/snapshots/final/ledger-001.json"
        },
        {
          "label": "Final budget",
          "href": "/break-hubu/evidence/v0.2.4/ep03-crash-recovery/snapshots/final/budgets.json"
        },
        {
          "label": "Run provenance",
          "href": "/break-hubu/evidence/v0.2.4/ep03-crash-recovery/run.json"
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
      ],
      "visual": {
        "kind": "burst",
        "allowed": 33,
        "denied": 67,
        "errors": 0,
        "reservedMicrousd": 990000,
        "consumedMicrousd": 990000,
        "remainingMicrousd": 10000,
        "source": {
          "label": "Captured request trace",
          "href": "/break-hubu/evidence/v0.2.4/ep04-many-clients/requests.jsonl"
        }
      },
      "terminal": "[2026-10-22T02:05:03.917504+00:00] $ /private/tmp/break-hubu-public-cache/target/release/hubu --url http://127.0.0.1:52039 budget list\n[2026-10-22T02:05:03.923240+00:00]   budget_id: bgt_gpcm6mns6tkb  agent_id: agt_qgxbfqebvp11  status: active\n    limit: $1.00  consumed: $0.99  frozen: $0.00  remaining: $0.01\n    current_version_id: bgv_zxa5jw5wbjfq  current_revision: 1\n    period: 2026-10-21T19:05:03.184779-07:00 -> 2998-12-31T16:00:00-08:00\nCLI exit=0\n",
      "evidence": [
        {
          "label": "Full transcript",
          "href": "/break-hubu/evidence/v0.2.4/ep04-many-clients/transcript.txt"
        },
        {
          "label": "Request trace",
          "href": "/break-hubu/evidence/v0.2.4/ep04-many-clients/requests.jsonl"
        },
        {
          "label": "Verification result",
          "href": "/break-hubu/evidence/v0.2.4/ep04-many-clients/result.json"
        },
        {
          "label": "Final ledger (page 1)",
          "href": "/break-hubu/evidence/v0.2.4/ep04-many-clients/snapshots/final/ledger-001.json"
        },
        {
          "label": "Final budget",
          "href": "/break-hubu/evidence/v0.2.4/ep04-many-clients/snapshots/final/budgets.json"
        },
        {
          "label": "Run provenance",
          "href": "/break-hubu/evidence/v0.2.4/ep04-many-clients/run.json"
        }
      ]
    }
  }
};
