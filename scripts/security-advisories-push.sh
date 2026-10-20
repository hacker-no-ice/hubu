#!/usr/bin/env bash
# Publishes data only. This privileged job must never build candidate packages.
set -euo pipefail

: "${REPORT_DIR:?REPORT_DIR is required}"
: "${DEFAULT_BRANCH:?DEFAULT_BRANCH is required}"
: "${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"
branch=codex/security-advisories
validation=$(tr -d '\n' < "$REPORT_DIR/validation")
case "$validation" in
  passed|failed|not_run) ;;
  *) echo 'Invalid candidate validation status' >&2; exit 1 ;;
esac
echo "validation=$validation" >> "$GITHUB_OUTPUT"
echo 'changed=false' >> "$GITHUB_OUTPUT"
echo 'pushed=false' >> "$GITHUB_OUTPUT"
echo 'updated=false' >> "$GITHUB_OUTPUT"
[ "$validation" = passed ] || exit 0

source_sha=$(tr -d '\n' < "$REPORT_DIR/source-sha")
[[ "$source_sha" =~ ^[0-9a-f]{40}$ ]] || { echo 'Invalid source commit' >&2; exit 1; }
[ "$(git rev-parse HEAD)" = "$source_sha" ] || { echo 'Source commit does not match the validated checkout' >&2; exit 1; }
git diff --quiet && git diff --cached --quiet || {
  echo 'Publication requires a clean tracked checkout; refusing to include other changes.' >&2
  exit 1
}
git fetch --no-tags origin "$DEFAULT_BRANCH"
[ "$(git rev-parse FETCH_HEAD)" = "$source_sha" ] || {
  echo 'Default branch advanced during validation; the next scan will retry.' >&2
  exit 1
}

cp "$REPORT_DIR/Cargo.lock" Cargo.lock
git diff --quiet -- Cargo.lock && exit 0
echo 'changed=true' >> "$GITHUB_OUTPUT"
existing=$(git ls-remote --heads origin "refs/heads/$branch" | cut -f1)
if [ -n "$existing" ]; then
  git fetch --no-tags origin "$existing"
  # This reserved branch belongs only to this bot. Do not replace human work.
  [ "$(git show -s --format='%ae' "$existing")" = '41898282+github-actions[bot]@users.noreply.github.com' ] || {
    echo 'Reserved remediation branch has a non-bot commit; human review required.' >&2
    exit 1
  }
  comparison_base=$(git merge-base "$source_sha" "$existing")
  if [ -n "$(git diff --name-only "$comparison_base" "$existing" -- . ':!Cargo.lock')" ]; then
    echo 'Reserved remediation branch contains changes outside Cargo.lock; human review required.' >&2
    exit 1
  fi
  if git show "$existing:Cargo.lock" | cmp -s - Cargo.lock && git merge-base --is-ancestor "$source_sha" "$existing"; then
    echo 'pushed=true' >> "$GITHUB_OUTPUT"
    echo "verified_sha=$existing" >> "$GITHUB_OUTPUT"
    exit 0
  fi
fi

git config user.name 'github-actions[bot]'
git config user.email '41898282+github-actions[bot]@users.noreply.github.com'
git add -- Cargo.lock
git commit -m 'fix(deps): apply validated compatible security updates'
git push "--force-with-lease=refs/heads/$branch:$existing" origin "HEAD:refs/heads/$branch"
echo 'pushed=true' >> "$GITHUB_OUTPUT"
echo 'updated=true' >> "$GITHUB_OUTPUT"
echo "verified_sha=$(git rev-parse HEAD)" >> "$GITHUB_OUTPUT"
