import type { ReleaseReport } from "../report-types";
export type { EpisodeId, EpisodeResult, EvidenceVisual, Verdict } from "../report-types";

// Generated from captured public evidence by break-hubu/render-report.py.
export const report: ReleaseReport = {
  "release": "v0.2.5",
  "sourceCommit": "b546c44af50fd3c34b34addd63d1d7a6a3a44f38",
  "kitsCommit": "fb40084a4993f12f5cb9fa7036c5a028a7d91d68",
  "recordedAt": "2026-11-07 UTC",
  "host": "macOS-15.8.2-arm64-arm-64bit-Mach-O",
  "evidenceBundle": {
    "label": "Download all evidence (ZIP)",
    "href": "/break-hubu/evidence/v0.2.5/evidence.zip"
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
          "href": "/break-hubu/evidence/v0.2.5/ep01-runaway/requests.jsonl"
        }
      },
      "terminal": "[2026-11-07T00:16:00.504542+00:00] $ /Users/runner/work/_temp/build-cache/v0.2.5/target/release/hubu --url http://127.0.0.1:49177 budget list\n[2026-11-07T00:16:00.515007+00:00]   budget_id: bgt_byzspttk22ps  agent_id: agt_1k1s05ft20m3  status: exhausted\n    limit: $0.10  consumed: $0.10  frozen: $0.00  remaining: $0.00\n    current_version_id: bgv_jn0xxxkh96wh  current_revision: 1\n    period: 2026-11-07T00:15:58.894736+00:00 -> 2999-01-01T00:00:00+00:00\nCLI exit=0\n",
      "evidence": [
        {
          "label": "Full transcript",
          "href": "/break-hubu/evidence/v0.2.5/ep01-runaway/transcript.txt"
        },
        {
          "label": "Request trace",
          "href": "/break-hubu/evidence/v0.2.5/ep01-runaway/requests.jsonl"
        },
        {
          "label": "Verification result",
          "href": "/break-hubu/evidence/v0.2.5/ep01-runaway/result.json"
        },
        {
          "label": "Final ledger (page 1)",
          "href": "/break-hubu/evidence/v0.2.5/ep01-runaway/snapshots/final/ledger-001.json"
        },
        {
          "label": "Final budget",
          "href": "/break-hubu/evidence/v0.2.5/ep01-runaway/snapshots/final/budgets.json"
        },
        {
          "label": "Run provenance",
          "href": "/break-hubu/evidence/v0.2.5/ep01-runaway/run.json"
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
          "href": "/break-hubu/evidence/v0.2.5/ep02-last-cent/snapshots/final/authorizations-001.json"
        }
      },
      "terminal": "[2026-11-07T00:16:34.544903+00:00] $ /Users/runner/work/_temp/build-cache/v0.2.5/target/release/hubu --url http://127.0.0.1:49343 budget list\n[2026-11-07T00:16:34.552017+00:00]   budget_id: bgt_jqavmrq9hevm  agent_id: agt_4ra6dpfc671g  status: exhausted\n    limit: $0.01  consumed: $0.03  frozen: $0.00  remaining: -$0.02\n    current_version_id: bgv_jyraxj1cf3gh  current_revision: 1\n    period: 2026-11-07T00:16:03.999158+00:00 -> 2999-01-01T00:00:00+00:00\nCLI exit=0\n",
      "evidence": [
        {
          "label": "Full transcript",
          "href": "/break-hubu/evidence/v0.2.5/ep02-last-cent/transcript.txt"
        },
        {
          "label": "Request trace",
          "href": "/break-hubu/evidence/v0.2.5/ep02-last-cent/requests.jsonl"
        },
        {
          "label": "Verification result",
          "href": "/break-hubu/evidence/v0.2.5/ep02-last-cent/result.json"
        },
        {
          "label": "Final ledger (page 1)",
          "href": "/break-hubu/evidence/v0.2.5/ep02-last-cent/snapshots/final/ledger-001.json"
        },
        {
          "label": "Final budget",
          "href": "/break-hubu/evidence/v0.2.5/ep02-last-cent/snapshots/final/budgets.json"
        },
        {
          "label": "Run provenance",
          "href": "/break-hubu/evidence/v0.2.5/ep02-last-cent/run.json"
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
            "elapsedSeconds": 0.002235,
            "detail": "Durable synthetic receipt; no provider call."
          },
          {
            "label": "Fixture saved",
            "elapsedSeconds": 0.00263,
            "detail": "Durable synthetic receipt; no provider call."
          },
          {
            "label": "Hubu process killed",
            "elapsedSeconds": 0.053736,
            "detail": "SIGKILL; process exit -9."
          },
          {
            "label": "Hubu restarted",
            "elapsedSeconds": 0.207098,
            "detail": "Same released binary and database."
          },
          {
            "label": "Public state after restart",
            "elapsedSeconds": 0.208741,
            "detail": "Frozen $0.01; consumed $0.00."
          },
          {
            "label": "Public state after lease expiry",
            "elapsedSeconds": 30.089313,
            "detail": "Frozen $0.01; consumed $0.00."
          },
          {
            "label": "Owner reconciliation",
            "elapsedSeconds": 30.186757,
            "detail": "CLI exit 0."
          },
          {
            "label": "Identical owner retry",
            "elapsedSeconds": 30.227509,
            "detail": "CLI exit 0."
          },
          {
            "label": "Hubu restarted",
            "elapsedSeconds": 30.438093,
            "detail": "Same released binary and database."
          },
          {
            "label": "Final public state",
            "elapsedSeconds": 30.440843,
            "detail": "Frozen $0.00; consumed $0.01."
          }
        ],
        "source": {
          "label": "Process events and public state exports (ZIP)",
          "href": "/break-hubu/evidence/v0.2.5/evidence.zip"
        }
      },
      "terminal": "[2026-11-07T00:17:05.972989+00:00] $ /Users/runner/work/_temp/build-cache/v0.2.5/target/release/hubu --url http://127.0.0.1:49427 budget list\n[2026-11-07T00:17:05.990334+00:00]   budget_id: bgt_b9pkng1n3x7d  agent_id: agt_9wf1mfvt5bb2  status: exhausted\n    limit: $0.01  consumed: $0.01  frozen: $0.00  remaining: $0.00\n    current_version_id: bgv_e7rfdmskwtbs  current_revision: 1\n    period: 2026-11-07T00:16:35.336853+00:00 -> 2999-01-01T00:00:00+00:00\nCLI exit=0\n",
      "evidence": [
        {
          "label": "Full transcript",
          "href": "/break-hubu/evidence/v0.2.5/ep03-crash-recovery/transcript.txt"
        },
        {
          "label": "Request trace",
          "href": "/break-hubu/evidence/v0.2.5/ep03-crash-recovery/requests.jsonl"
        },
        {
          "label": "Verification result",
          "href": "/break-hubu/evidence/v0.2.5/ep03-crash-recovery/result.json"
        },
        {
          "label": "Final ledger (page 1)",
          "href": "/break-hubu/evidence/v0.2.5/ep03-crash-recovery/snapshots/final/ledger-001.json"
        },
        {
          "label": "Final budget",
          "href": "/break-hubu/evidence/v0.2.5/ep03-crash-recovery/snapshots/final/budgets.json"
        },
        {
          "label": "Run provenance",
          "href": "/break-hubu/evidence/v0.2.5/ep03-crash-recovery/run.json"
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
          "href": "/break-hubu/evidence/v0.2.5/ep04-many-clients/requests.jsonl"
        }
      },
      "terminal": "[2026-11-07T00:17:08.282649+00:00] $ /Users/runner/work/_temp/build-cache/v0.2.5/target/release/hubu --url http://127.0.0.1:49516 budget list\n[2026-11-07T00:17:08.290722+00:00]   budget_id: bgt_d4d8bsq3kzr5  agent_id: agt_q5tzwg9haafv  status: active\n    limit: $1.00  consumed: $0.99  frozen: $0.00  remaining: $0.01\n    current_version_id: bgv_cwkd66s2gcr1  current_revision: 1\n    period: 2026-11-07T00:17:06.777004+00:00 -> 2999-01-01T00:00:00+00:00\nCLI exit=0\n",
      "evidence": [
        {
          "label": "Full transcript",
          "href": "/break-hubu/evidence/v0.2.5/ep04-many-clients/transcript.txt"
        },
        {
          "label": "Request trace",
          "href": "/break-hubu/evidence/v0.2.5/ep04-many-clients/requests.jsonl"
        },
        {
          "label": "Verification result",
          "href": "/break-hubu/evidence/v0.2.5/ep04-many-clients/result.json"
        },
        {
          "label": "Final ledger (page 1)",
          "href": "/break-hubu/evidence/v0.2.5/ep04-many-clients/snapshots/final/ledger-001.json"
        },
        {
          "label": "Final budget",
          "href": "/break-hubu/evidence/v0.2.5/ep04-many-clients/snapshots/final/budgets.json"
        },
        {
          "label": "Run provenance",
          "href": "/break-hubu/evidence/v0.2.5/ep04-many-clients/run.json"
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
      ],
      "visual": null,
      "terminal": "[2026-11-09T19:25:42.119592+00:00] $ /Users/runner/work/_temp/build-cache/v0.2.5/target/release/hubu --url http://127.0.0.1:49179 budget list\n[2026-11-09T19:25:42.135346+00:00]   budget_id: bgt_dy822mba4adc  agent_id: agt_bb1rm0wg6d34  status: active\n    limit: $5.00  consumed: $0.01  frozen: $0.00  remaining: $4.99\n    current_version_id: bgv_8ake6mmt5z0e  current_revision: 1\n    period: 2026-11-09T19:25:41.137906+00:00 -> 2999-01-01T00:00:00+00:00\nCLI exit=0\n",
      "evidence": [
        {
          "label": "Full transcript",
          "href": "/break-hubu/evidence/v0.2.5/ep05-denied-operation/transcript.txt"
        },
        {
          "label": "Request trace",
          "href": "/break-hubu/evidence/v0.2.5/ep05-denied-operation/requests.jsonl"
        },
        {
          "label": "Verification result",
          "href": "/break-hubu/evidence/v0.2.5/ep05-denied-operation/result.json"
        },
        {
          "label": "Final ledger (page 1)",
          "href": "/break-hubu/evidence/v0.2.5/ep05-denied-operation/snapshots/final/ledger-001.json"
        },
        {
          "label": "Final budget",
          "href": "/break-hubu/evidence/v0.2.5/ep05-denied-operation/snapshots/final/budgets.json"
        },
        {
          "label": "Run provenance",
          "href": "/break-hubu/evidence/v0.2.5/ep05-denied-operation/run.json"
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
      ],
      "visual": null,
      "terminal": "[2026-11-09T19:25:44.049497+00:00] $ /Users/runner/work/_temp/build-cache/v0.2.5/target/release/hubu --url http://127.0.0.1:49211 budget list\n[2026-11-09T19:25:44.059498+00:00]   budget_id: bgt_cnpmxpcp9c0s  agent_id: agt_cc50jwfcs68g  status: active\n    limit: $1.00  consumed: $0.10  frozen: $0.00  remaining: $0.90\n    current_version_id: bgv_wvbr2sya8c5c  current_revision: 1\n    period: 2026-11-09T19:25:43.737152+00:00 -> 2999-01-01T00:00:00+00:00\nCLI exit=0\n",
      "evidence": [
        {
          "label": "Full transcript",
          "href": "/break-hubu/evidence/v0.2.5/ep06-approval-reuse/transcript.txt"
        },
        {
          "label": "Request trace",
          "href": "/break-hubu/evidence/v0.2.5/ep06-approval-reuse/requests.jsonl"
        },
        {
          "label": "Verification result",
          "href": "/break-hubu/evidence/v0.2.5/ep06-approval-reuse/result.json"
        },
        {
          "label": "Final ledger (page 1)",
          "href": "/break-hubu/evidence/v0.2.5/ep06-approval-reuse/snapshots/final/ledger-001.json"
        },
        {
          "label": "Final budget",
          "href": "/break-hubu/evidence/v0.2.5/ep06-approval-reuse/snapshots/final/budgets.json"
        },
        {
          "label": "Run provenance",
          "href": "/break-hubu/evidence/v0.2.5/ep06-approval-reuse/run.json"
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
      ],
      "visual": null,
      "terminal": "[2026-11-09T19:25:45.178839+00:00] $ /Users/runner/work/_temp/build-cache/v0.2.5/target/release/hubu --url http://127.0.0.1:49246 budget list\n[2026-11-09T19:25:45.189681+00:00]   budget_id: bgt_y7mzgfvmhmt6  agent_id: agt_tvrjgx7ysc1q  status: active\n    limit: $0.10  consumed: $0.00  frozen: $0.00  remaining: $0.10\n    current_version_id: bgv_vqkv3cyspaas  current_revision: 1\n    period: 2026-11-09T19:25:44.855615+00:00 -> 2999-01-01T00:00:00+00:00\n  budget_id: bgt_apnjt2ke8xma  agent_id: agt_dvyv0fck078r  status: active\n    limit: $1.00  consumed: $0.05  frozen: $0.00  remaining: $0.95\n    current_version_id: bgv_270gkwn4t15t  current_revision: 1\n    period: 2026-11-09T19:25:44.902986+00:00 -> 2999-01-01T00:00:00+00:00\nCLI exit=0\n",
      "evidence": [
        {
          "label": "Full transcript",
          "href": "/break-hubu/evidence/v0.2.5/ep07-impersonate-agent/transcript.txt"
        },
        {
          "label": "Request trace",
          "href": "/break-hubu/evidence/v0.2.5/ep07-impersonate-agent/requests.jsonl"
        },
        {
          "label": "Verification result",
          "href": "/break-hubu/evidence/v0.2.5/ep07-impersonate-agent/result.json"
        },
        {
          "label": "Final ledger (page 1)",
          "href": "/break-hubu/evidence/v0.2.5/ep07-impersonate-agent/snapshots/final/ledger-001.json"
        },
        {
          "label": "Final budget",
          "href": "/break-hubu/evidence/v0.2.5/ep07-impersonate-agent/snapshots/final/budgets.json"
        },
        {
          "label": "Run provenance",
          "href": "/break-hubu/evidence/v0.2.5/ep07-impersonate-agent/run.json"
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
      ],
      "visual": null,
      "terminal": "[2026-11-09T19:25:46.616650+00:00] $ /Users/runner/work/_temp/build-cache/v0.2.5/target/release/hubu --url http://127.0.0.1:49276 budget list\n[2026-11-09T19:25:46.626024+00:00]   budget_id: bgt_zsw804p4vg92  agent_id: agt_am2ah30db5g5  status: exhausted\n    limit: $0.10  consumed: $0.00  frozen: $0.10  remaining: $0.00\n    current_version_id: bgv_t038e6t2bh59  current_revision: 1\n    period: 2026-11-09T19:25:46.236680+00:00 -> 2999-01-01T00:00:00+00:00\nCLI exit=0\n",
      "evidence": [
        {
          "label": "Full transcript",
          "href": "/break-hubu/evidence/v0.2.5/ep08-cross-session/transcript.txt"
        },
        {
          "label": "Request trace",
          "href": "/break-hubu/evidence/v0.2.5/ep08-cross-session/requests.jsonl"
        },
        {
          "label": "Verification result",
          "href": "/break-hubu/evidence/v0.2.5/ep08-cross-session/result.json"
        },
        {
          "label": "Final ledger (page 1)",
          "href": "/break-hubu/evidence/v0.2.5/ep08-cross-session/snapshots/final/ledger-001.json"
        },
        {
          "label": "Final budget",
          "href": "/break-hubu/evidence/v0.2.5/ep08-cross-session/snapshots/final/budgets.json"
        },
        {
          "label": "Run provenance",
          "href": "/break-hubu/evidence/v0.2.5/ep08-cross-session/run.json"
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
      ],
      "visual": null,
      "terminal": "[2026-11-09T19:25:47.609482+00:00] $ /Users/runner/work/_temp/build-cache/v0.2.5/target/release/hubu --url http://127.0.0.1:49302 budget list\n[2026-11-09T19:25:47.619361+00:00]   budget_id: bgt_0peb3fzcs59y  agent_id: agt_zxgbtqxyq70r  status: active\n    limit: $100.00  consumed: $0.00  frozen: $0.50  remaining: $99.50\n    current_version_id: bgv_hw7cxe9qrp8s  current_revision: 2\n    period: 2026-11-09T19:25:47.358849+00:00 -> 2999-01-01T00:00:00+00:00\nCLI exit=0\n",
      "evidence": [
        {
          "label": "Full transcript",
          "href": "/break-hubu/evidence/v0.2.5/ep09-raise-own-budget/transcript.txt"
        },
        {
          "label": "Request trace",
          "href": "/break-hubu/evidence/v0.2.5/ep09-raise-own-budget/requests.jsonl"
        },
        {
          "label": "Verification result",
          "href": "/break-hubu/evidence/v0.2.5/ep09-raise-own-budget/result.json"
        },
        {
          "label": "Final ledger (page 1)",
          "href": "/break-hubu/evidence/v0.2.5/ep09-raise-own-budget/snapshots/final/ledger-001.json"
        },
        {
          "label": "Final budget",
          "href": "/break-hubu/evidence/v0.2.5/ep09-raise-own-budget/snapshots/final/budgets.json"
        },
        {
          "label": "Run provenance",
          "href": "/break-hubu/evidence/v0.2.5/ep09-raise-own-budget/run.json"
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
      ],
      "visual": null,
      "terminal": "[2026-11-09T19:25:48.437140+00:00] $ /Users/runner/work/_temp/build-cache/v0.2.5/target/release/hubu --url http://127.0.0.1:49327 budget list\n[2026-11-09T19:25:48.446609+00:00]   budget_id: bgt_b9y0vc69et1j  agent_id: agt_j87w30ghzb9f  status: active\n    limit: $1.00  consumed: $0.00  frozen: $0.00  remaining: $1.00\n    current_version_id: bgv_70epyrgr340b  current_revision: 1\n    period: 2026-11-09T19:25:48.244308+00:00 -> 2999-01-01T00:00:00+00:00\nCLI exit=0\n",
      "evidence": [
        {
          "label": "Full transcript",
          "href": "/break-hubu/evidence/v0.2.5/ep10-self-approval/transcript.txt"
        },
        {
          "label": "Request trace",
          "href": "/break-hubu/evidence/v0.2.5/ep10-self-approval/requests.jsonl"
        },
        {
          "label": "Verification result",
          "href": "/break-hubu/evidence/v0.2.5/ep10-self-approval/result.json"
        },
        {
          "label": "Final ledger (page 1)",
          "href": "/break-hubu/evidence/v0.2.5/ep10-self-approval/snapshots/final/ledger-001.json"
        },
        {
          "label": "Final budget",
          "href": "/break-hubu/evidence/v0.2.5/ep10-self-approval/snapshots/final/budgets.json"
        },
        {
          "label": "Run provenance",
          "href": "/break-hubu/evidence/v0.2.5/ep10-self-approval/run.json"
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
      ],
      "visual": null,
      "terminal": "[2026-11-09T19:26:19.324397+00:00] $ /Users/runner/work/_temp/build-cache/v0.2.5/target/release/hubu --url http://127.0.0.1:49355 budget list\n[2026-11-09T19:26:19.342181+00:00]   budget_id: bgt_kh36xm1h8grp  agent_id: agt_f424setntr96  status: active\n    limit: $0.10  consumed: $0.09  frozen: $0.00  remaining: $0.01\n    current_version_id: bgv_apb7c780sja1  current_revision: 1\n    period: 2026-11-09T19:25:48.984366+00:00 -> 2999-01-01T00:00:00+00:00\nCLI exit=0\n",
      "evidence": [
        {
          "label": "Full transcript",
          "href": "/break-hubu/evidence/v0.2.5/ep11-rewrite-ledger/transcript.txt"
        },
        {
          "label": "Request trace",
          "href": "/break-hubu/evidence/v0.2.5/ep11-rewrite-ledger/requests.jsonl"
        },
        {
          "label": "Verification result",
          "href": "/break-hubu/evidence/v0.2.5/ep11-rewrite-ledger/result.json"
        },
        {
          "label": "Final ledger (page 1)",
          "href": "/break-hubu/evidence/v0.2.5/ep11-rewrite-ledger/snapshots/final/ledger-001.json"
        },
        {
          "label": "Final budget",
          "href": "/break-hubu/evidence/v0.2.5/ep11-rewrite-ledger/snapshots/final/budgets.json"
        },
        {
          "label": "Run provenance",
          "href": "/break-hubu/evidence/v0.2.5/ep11-rewrite-ledger/run.json"
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
      ],
      "visual": null,
      "terminal": "[2026-11-09T19:26:20.741618+00:00] $ /Users/runner/work/_temp/build-cache/v0.2.5/target/release/hubu --url http://127.0.0.1:49432 budget list\n[2026-11-09T19:26:20.751340+00:00]   budget_id: bgt_s1drfzh10hnw  agent_id: agt_3mb2sm7jjxta  status: active\n    limit: $1.00  consumed: $0.50  frozen: $0.00  remaining: $0.50\n    current_version_id: bgv_dw70p469aw5j  current_revision: 1\n    period: 2026-11-09T19:26:20.422275+00:00 -> 2999-01-01T00:00:00+00:00\nCLI exit=0\n",
      "evidence": [
        {
          "label": "Full transcript",
          "href": "/break-hubu/evidence/v0.2.5/ep12-misstated-approval/transcript.txt"
        },
        {
          "label": "Request trace",
          "href": "/break-hubu/evidence/v0.2.5/ep12-misstated-approval/requests.jsonl"
        },
        {
          "label": "Verification result",
          "href": "/break-hubu/evidence/v0.2.5/ep12-misstated-approval/result.json"
        },
        {
          "label": "Final ledger (page 1)",
          "href": "/break-hubu/evidence/v0.2.5/ep12-misstated-approval/snapshots/final/ledger-001.json"
        },
        {
          "label": "Final budget",
          "href": "/break-hubu/evidence/v0.2.5/ep12-misstated-approval/snapshots/final/budgets.json"
        },
        {
          "label": "Run provenance",
          "href": "/break-hubu/evidence/v0.2.5/ep12-misstated-approval/run.json"
        }
      ]
    }
  },
  "supplements": [
    {
      "kitsCommit": "de1104f2b343e09713966c58a037e00d27b8de7d",
      "recordedAt": "2026-11-09 UTC",
      "host": "macOS-15.8.2-arm64-arm-64bit-Mach-O",
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
