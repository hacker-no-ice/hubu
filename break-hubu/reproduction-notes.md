# v0.2.4 reproduction and qualification notes

The recorded runs used a clean clone of kits commit
`f280b3dc9488040ff9e6ea9b7b42e3447f7b1021`. The scripts build the older released
source `5344ab565171e1aa3d3ec628641cdbfce18372af` in a separate checkout.
The website and measured release card are added afterward, so they do not need
to exist at the kits commit to reproduce the experiments.

The recording command used a fresh build cache and the host's
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

| Attempt | What happened | Harness correction |
| --- | --- | --- |
| 1 | The tool sandbox blocked an operation before setup completed. | Run the isolated local server with the required permissions. |
| 2 | Public policy inspection found no assignment for the requested agent scope. | Apply the fixture policy explicitly to the registered agent. |
| 3 | Authorization rejected an invented fixture execution-scope identifier. | Use the fixture entry returned by the released public executor catalog. |

The Python fixture sends the external-executor HTTP calls. Its catalog entry has
the metadata value `executor:gongbu:image`. No Gongbu process starts.

## Verifier qualification

Saved-evidence checks were also exercised with explicitly altered local copies
of development captures: missing final exports, unresolved settlement responses,
expired workload, incorrect financial balances, missing client/barrier events,
and a known above-budget reservation followed by missing evidence. These copies
are verifier tests, not observations of Hubu, and are not used in report plots.
Missing/ambiguous evidence yields **not tested**; a demonstrated contradiction
survives a subsequent missing step. Optimized Python (`-O`) cannot yield a pass.

The completed clean run returned exit 0. EP01 and EP03 held, while EP02 and
EP04 held with caveats. The saved outputs re-verified with the pinned kits code.

The independent verifiers inspect saved HTTP and CLI data. They check complete
pagination, exact signed charges, balanced ledger entries, linkage across
authorization, claim, and settlement records, coverage, and fixed operator
configuration.
