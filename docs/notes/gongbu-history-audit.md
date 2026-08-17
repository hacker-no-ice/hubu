# Gongbu Public-History Audit (HUB-79)

Date: 2026-08-17 (America/Los_Angeles)

## Decision

**Go for an unsquashed, history-preserving import of the exact audited
`hacker-no-ice/gongbu` `main` tip
`b7132f647f14cc0d527384150341e7b42cbed1b4`.** No history sanitization,
credential rotation, or old-to-new commit mapping is indicated by this audit.

This decision applies only to the object graph reachable from that exact commit.
HUB-80 must re-read the remote `main` tip immediately before importing and stop
if it differs. The import must also retain Hubu's `LICENSE-MIT` and
`LICENSE-APACHE` files. A changed source tip, a different source ref, or an
import of additional refs requires a new audit of the added objects.

No Gongbu commit, tree, blob, or working-tree file was added to Hubu as part of
HUB-79. This note and its index link are the only repository changes.

## Reviewed scope

| Item | Audited value |
| --- | --- |
| Source repository | `https://github.com/hacker-no-ice/gongbu.git` |
| Reviewed ref | `refs/heads/main` only |
| Remote and local source tip | `b7132f647f14cc0d527384150341e7b42cbed1b4` |
| Root commit | `e68da51d5e8ee1994f67a607005bac6a16a3db15` |
| Reachable commits | 146 (34 merges) |
| Unique reachable blob payloads | 428 |
| Historical paths | 67 (54 files at the audited tip) |
| Aggregate unique-blob bytes | 10,405,795 |
| Largest blob | 87,823 bytes (`Cargo.lock`) |
| Hubu base observed for this report | `616bcf787a7b6f9f084cd47a1446224ad5dad27f` |

The Linear issue described 179 known commits. Fresh enumeration of the exact
remote `main` tip produced 146 reachable commits. This is not a partial or
shallow clone: the isolated clone had one source branch, no tags, and no shallow
boundary. The remaining difference is therefore an estimate/ref-scope
discrepancy, not omitted `main` history. Other Gongbu refs and unreachable local
objects were intentionally excluded because they are not part of the proposed
`main` import.

## Automated full-history scan

The audit used Gitleaks 8.29.1 for both Git-history and independently enumerated
blob scans. The official Darwin arm64 release archive was verified against the
release checksum before execution:

```text
gitleaks_8.29.1_darwin_arm64.tar.gz
SHA-256 69836c841d7e648fb30ff4846f8c3587855c5754ed02b8510caaf6008f65d177
```

Reproducible commands (replace the working paths as needed):

```sh
git ls-remote https://github.com/hacker-no-ice/gongbu.git refs/heads/main
git clone --single-branch --branch main --no-tags \
  https://github.com/hacker-no-ice/gongbu.git gongbu-main
git -C gongbu-main rev-parse HEAD
git -C gongbu-main rev-list --count HEAD

gitleaks git gongbu-main --redact=100 --report-format=json \
  --report-path=gitleaks-history.json --no-banner

git -C gongbu-main rev-list --objects HEAD \
  | git -C gongbu-main cat-file \
      --batch-check='%(objectname) %(objecttype) %(objectsize) %(rest)'
# Export each unique object reported as a blob with `git cat-file blob`, then:
gitleaks dir all-blobs --redact=100 --report-format=json \
  --report-path=gitleaks-blobs.json --no-banner

git -C gongbu-main fsck --strict --connectivity-only --no-dangling HEAD
```

Results:

- History scan: zero findings; Gitleaks reported 111 scanned revisions and
  approximately 1.31 MB of diff content.
- Independent reachable-blob scan: zero findings across all 428 unique blob
  payloads and 10.41 MB of content.
- Reachable-object connectivity check: passed.

Gitleaks' 111-revision progress count is lower than Git's 146 reachable commits.
The independent blob scan removes ambiguity about content coverage, while the
separate metadata review below covers all 146 commit messages and identities.
No scan report is committed because both reports were empty and temporary.

## Targeted manual review

The manual pass enumerated every historical path and every unique reachable
blob, then reviewed redacted match metadata rather than emitting candidate
values. It covered private-key headers; common cloud, GitHub, Google, Slack, and
Stripe token formats; generic secret/token/password/credential assignments;
URLs and query-key names; URI user-info; RFC 1918, loopback, `.local`,
`.internal`, and `.corp` hosts; absolute user-home paths; suspicious filenames;
file type and size; commit subjects/bodies; author and committer fields; and
license/provenance markers.

### Secrets, configuration, and operational details

- No private-key marker or known token format was present.
- Generic secret-keyword candidates were false positives in security code,
  redaction tests, typed credential references, environment-variable names, and
  explicit fixtures/placeholders. Gitleaks found none of them to be secrets.
- URL query keys and user-info appeared only in redaction/validation tests or
  reserved synthetic hosts. No value was retained in this report.
- Hosts were public provider/documentation endpoints, GitHub dependency sources,
  loopback addresses, or reserved `.example`/`.test` fixtures. The sole
  `.internal` host is Google's documented metadata-service hostname used by the
  GCP secret-provider implementation, not a private operator endpoint.
- No RFC 1918 endpoint, operator hostname, absolute `/Users/...` or `/home/...`
  path, checked-in database, `.env` file, key store, certificate, or live
  operator configuration was found.
- Historical operator guidance describes schemas and local keychain/secret
  manager use, but contains only examples and public service details. It does
  not expose a deployed environment or credential value.

### Binary and large-file review

Content classification found 428 text or JSON blobs and no binary payloads.
No blob is at least 100 KB or 1 MB; the maximum is the 87,823-byte lockfile
version noted above. The historical path set contains source, manifests,
documentation, workflow configuration, SQL, examples, and one JSON fixture—no
archives, generated artifacts, media, executables, or vendor trees.

### Commit messages and identity metadata

All 146 commit subjects and bodies were checked for secret markers, known token
formats, URLs, control characters, and unusually long subjects. There were no
flags and no co-author trailers. Metadata is structurally valid and spans
2026-06-05 through 2026-08-13.

The history contains two author identities (`Yizheng Zhang` and
`hacker-no-ice`) using one `outlook.com` address, and three committer identities
(`Yizheng Zhang`, `hacker-no-ice`, and GitHub) using `outlook.com` or
`github.com` addresses. These are consistent with the repository owner and
GitHub merge commits; no unexpected external identity was found. Twenty-seven
commit objects contain embedded signatures. Signature validity was not
independently verified because GPG is unavailable in the audit environment;
signature verification is not relied on for the confidentiality decision.

### Licensing and provenance

- Every reachable commit declares `MIT OR Apache-2.0` in the root workspace
  manifest, and every crate manifest present in each commit inherits or declares
  a license field.
- Gongbu history contains no license text file. Hubu already carries the
  matching `LICENSE-MIT` and `LICENSE-APACHE` texts, which must remain in the
  combined repository.
- No vendored dependency source, copied binary, third-party asset, attribution
  notice, or source/provenance claim requiring preservation was found in the
  67-path history. `Cargo.lock` records registry dependencies but does not vendor
  their source into the import.
- Commit identity and repository history are consistent with first-party work.
  This is a technical provenance review, not a legal ownership attestation.

## Import recommendation for HUB-80

Preserve all Gongbu commit IDs and parent relationships. A suitable strategy is
to fetch the exact SHA into a dedicated remote-tracking ref and use a non-squash
subtree/read-tree merge that adds the Gongbu tree under the selected Hubu
workspace prefix. For example, once HUB-80 selects and prepares the destination:

```sh
git remote add gongbu https://github.com/hacker-no-ice/gongbu.git
git fetch --no-tags gongbu \
  refs/heads/main:refs/remotes/gongbu/audited-main
if test "$(git rev-parse refs/remotes/gongbu/audited-main)" != \
  b7132f647f14cc0d527384150341e7b42cbed1b4; then
  echo "Gongbu main does not match the audited SHA; stop the import." >&2
  exit 1
fi
git subtree add --prefix=<approved-gongbu-prefix> \
  refs/remotes/gongbu/audited-main
git merge-base --is-ancestor \
  b7132f647f14cc0d527384150341e7b42cbed1b4 HEAD
```

Do not pass `--squash`, run `git filter-repo`, or copy only the tip tree. HUB-80
should separately resolve workspace layout, root manifest, README, CI, and
repository-link changes; those implementation choices are outside this audit.
The import PR should show the audited Gongbu SHA as an ancestor and repeat a
Gitleaks scan on the combined object graph before approval.

## Residual limitations and stop conditions

- Secret detection is signature, pattern, and entropy based; no scanner can
  prove that an arbitrary value is harmless. The independent blob enumeration
  and targeted review reduce but do not eliminate that residual risk.
- Encrypted or encoded secrets can evade general scanners. No archive or binary
  blob was present, and known credential encodings/formats were checked.
- The audit covers only objects reachable from the recorded `main` SHA. It does
  not approve tags, other branches, reflogs, or unreachable objects from a local
  clone.
- Author metadata supports, but cannot legally prove, copyright ownership.
- If the remote SHA changes, the combined scan finds a secret, an unexpected
  ref is proposed, or ownership/licensing is disputed, stop the import and seek
  user judgment before rotation, rewriting, filtering, or publication.
