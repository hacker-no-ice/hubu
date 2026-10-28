# v0.2.4 reproduction and qualification notes

The published evidence uses a clean public clone of kits commit
`f280b3dc9488040ff9e6ea9b7b42e3447f7b1021`. The scripts build the older released
source `5344ab565171e1aa3d3ec628641cdbfce18372af` in a separate checkout.
The website and measured release card are added afterward, so they do not need
to exist at the kits commit to reproduce the experiments.

The recording command uses a new build target/cache directory, with the host's
existing Rustup installation and Cargo download cache. This is not an estimate
of download/build time on another machine. The experiment itself uses loopback;
source, release-metadata and dependency acquisition need network access.

```sh
git -c credential.helper= -c http.extraheader= clone --single-branch \
  --branch codex/break-hubu-proof-kits https://github.com/hacker-no-ice/hubu.git \
  /tmp/break-hubu-public-clone
git -C /tmp/break-hubu-public-clone checkout --detach \
  f280b3dc9488040ff9e6ea9b7b42e3447f7b1021
/tmp/break-hubu-public-clone/break-hubu/run-all.sh --release v0.2.4 \
  --cache /tmp/break-hubu-public-cache --output /tmp/break-hubu-public-v024
```

Git credential helpers and extra HTTP headers were cleared for the clone.
The runner records host/tool versions, source identity, binary versions and
whether its own checkout is dirty. A complete published result must report
`kits_worktree_dirty: false` in all four `run.json` files.

## Development setup failures

These retained captures qualify the harness. They are not results for the four
financial claims, and no released product code was changed to repair them.

| Actual capture | What happened | Harness correction |
| --- | --- | --- |
| [Attempt 1](qualification/setup-attempt-1/result.json) | The tool sandbox blocked an operation before setup completed. | Run the authorized isolated local server with the required tool permissions. |
| [Attempt 2](qualification/setup-attempt-2/result.json) | Public policy inspection found no assignment for the requested agent scope. | Apply the fixture policy explicitly to the registered agent. |
| [Attempt 3](qualification/setup-attempt-3/result.json) | Authorization rejected an invented fixture execution-scope identifier. | Use the fixture entry returned by the released public executor catalog. |

The catalog's `executor:gongbu:image` value is metadata in this test. The Python
fixture sends the external-executor HTTP calls; no Gongbu process is started.
The corresponding actual terminal captures are retained next to each result.
They do not include credential files, private operation records or databases.

## Verifier qualification

Saved-evidence checks were also exercised with explicitly altered local copies
of development captures: missing final exports, unresolved settlement responses,
expired workload, incorrect financial balances, missing client/barrier events,
and a known above-budget reservation followed by missing evidence. These copies
are verifier tests, not observations of Hubu, and are not used in report plots.
Missing/ambiguous evidence yields **not tested**; a demonstrated contradiction
survives a subsequent missing step. Optimized Python (`-O`) cannot yield a pass.

The completed clean run returned exit 0: EP01/EP03 **holds**, EP02/EP04
**holds with caveat**. The copied publication files re-verified with the pinned
kits code. All six owned server PIDs had exited after the run. A literal scan
using the private runtime's credentials, continuation tokens and operation keys
found none of those values in the public files.

The independent verifiers inspect saved public data, not the driver's success
counters or SQLite. They check complete pagination, exact signed charges,
balanced ledger entries, authorization/claim/settlement linkage, coverage, and
fixed operator configuration. The [public evidence bundle](../site/public/break-hubu/evidence/v0.2.4/evidence.zip)
contains the recording used by the card, including raw transcripts and exports.
The public summary omits internal tracker references from the runner's two
explanatory gap annotations; measured results and raw protocol exports are unchanged.

## Publication

Source/site review precedes production publication. After the PR merges, verify
the live page and append the reviewed links/evidence attachment to the existing
v0.2.4 release without replacing its original assets. Until then the local
preview and PR are review artifacts; the website is not claimed to be deployed.
