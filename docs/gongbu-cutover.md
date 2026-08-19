# Gongbu-to-Hubu Cutover Runbook

This historical runbook recorded the repository migration after one immutable
unified Hubu canary proved the then-current six-binary distribution from an
exact commit on `main`. HUB-97 now packages four binaries and excludes the two
deprecated standalone MCP adapters. This record does not authorize a live
provider call or real spend.

## Historical ordered cutover gates

1. Record the exact `main` commit and confirm the unified CI jobs for workspace
   tests, Rust 1.88 MSRV, and repository security succeeded for that commit.
2. Dispatch the `canary` channel in [the release workflow](../.github/workflows/release.yml)
   with that exact 40-character `source_commit`.
3. Under the temporary zero-user pre-launch exception, wait for every required
   release check, both macOS native builds, publication, and both macOS
   published-asset smoke jobs to succeed.
4. Download `SHA256SUMS` plus both macOS Intel and Apple silicon archives. Verify
   the top-level archive checksum and each archive's internal checksums,
   manifest, provenance, legal files, lockfile, six version surfaces, Hubu
   health/version endpoints, and both separate MCP initialization surfaces.
5. Run the deterministic native packaged-stack smoke. It may start isolated
   Hubu and Gongbu processes, but it must not configure provider credentials,
   invoke a provider, or submit spend.
6. Complete the compatibility inventory below and record the evidence in
   Linear HUB-86 before changing the legacy repository.
7. The private `hacker-no-ice/gongbu` README was changed to a short archival
   pointer to `https://github.com/hacker-no-ice/hubu` while it was writable.
8. The legacy repository settings and contents were re-read before archival;
   the archived repository was then verified read-only and accessible.

The canary tag is `main-<full-source-commit>` and must never be moved or have an
asset replaced. Record the release URL, workflow run URL, source commit, product
version, both macOS archive names and SHA-256 values, and the result of every
smoke job in HUB-86. The Linux release targets must be restored and verified
before the first supported Linux user or public availability.

## Compatibility inventory

The cutover changes source ownership and release identity, not the runtime
contract. Confirm all of these before archiving the legacy repository:

| Surface | Required invariant |
| --- | --- |
| Binaries | Historical evidence used `hubu`, `hubu-server`, `hubu-mcp-server`, `gongbu-server`, and `gongbu-mcp`; current HUB-97 archives retain only `hubu`, `hubu-server`, `hubu-unified-mcp`, and `gongbu-server`. |
| Configuration | Existing Gongbu server JSON remains accepted; provider and pricing configuration schemas are not rewritten by the repository move. |
| Environment | Existing `GONGBU_*`, Hubu URL/token, provider-secret, artifact-root, and Temporal settings retain their meanings. Build-only product/source metadata remains separate from runtime secrets. |
| Databases | Hubu and Gongbu continue to use separate operator-selected SQLite files. Neither process opens or migrates the other's database. |
| Artifacts | Gongbu continues to own normalized artifact bytes beneath its configured root and stores storage-neutral keys rather than absolute paths. |
| Temporal | The `gongbu-executions` task queue and stable `gongbu-execution-{execution_id}` workflow IDs remain unchanged, preserving replay and recovery semantics. |
| Boundary | Hubu remains the control plane and Gongbu the execution plane over `hubu-spend-executor-v4.2`; credentials, provider calls, MCP surfaces, storage, and failure domains remain separate. |

Use the unified workspace checks, deterministic executor E2E, archive verifier,
and published release smoke as executable evidence. A live dogfood is optional
and requires separate credentials and explicit spend authorization.

## Independent rollback baseline

The last independent Hubu binary release before the migration is the immutable
prerelease `main-959018f1dacc5e80f60cec209da5a5b360e9f095`. Retain its GitHub
Release assets and `SHA256SUMS`; do not move its tag or replace its files.

Gongbu never published an independent GitHub Release or tag. Its attempted
release automation in pull request 30 was closed without merge. The truthful
legacy rollback input is therefore the exact audited source and lockfile at
`b7132f647f14cc0d527384150341e7b42cbed1b4`, not a retroactively manufactured
binary release. That commit is preserved unchanged as an ancestor of Hubu and
remains reachable from the archived private repository.

Sanity-check the rollback inputs before cutover:

```sh
git merge-base --is-ancestor \
  b7132f647f14cc0d527384150341e7b42cbed1b4 HEAD
git show b7132f647f14cc0d527384150341e7b42cbed1b4:Cargo.lock >/dev/null
gh release view \
  main-959018f1dacc5e80f60cec209da5a5b360e9f095 \
  --repo hacker-no-ice/hubu
```

If the unified canary regresses, stop stable promotion and keep the legacy
repository unarchived. Restore the Hubu consumer pin to the recorded independent
release tag and checksum. Rebuild Gongbu only from the pinned audited commit,
its exact `Cargo.lock`, and Rust 1.85, using the pre-cutover configuration,
database, artifact root, and Temporal namespace/task queue. Revert workspace
activation or release changes as ordinary commits if necessary; never rewrite
or remove the imported Gongbu history.
