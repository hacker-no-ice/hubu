#!/usr/bin/env bash
# Exercise the privileged data-only publisher against local bare Git remotes.
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
publisher="$script_dir/security-advisories-push.sh"
test_root=$(mktemp -d "${TMPDIR:-/tmp}/hubu-security-push.XXXXXX")
trap 'rm -rf "$test_root"' EXIT

# Keep local Git tests independent of user identities, hooks and credentials.
export GIT_CONFIG_NOSYSTEM=1
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_TERMINAL_PROMPT=0
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE
real_git=$(command -v git)
bot_email='41898282+github-actions[bot]@users.noreply.github.com'
fix_branch=codex/security-advisories
tests=0

fail() {
  echo "security publication integration test failed: $*" >&2
  if [ -f "${case_dir:-}/publisher.stderr" ]; then
    cat "$case_dir/publisher.stderr" >&2
  fi
  exit 1
}

write_lock() {
  local destination=$1
  local version=$2
  cat > "$destination" <<EOF
version = 4

[[package]]
name = "security-fixture"
version = "$version"
source = "registry+https://github.com/rust-lang/crates.io-index"
EOF
}

capture_expected_lock_hash() {
  EXPECTED_LOCK_SHA=$(python3 - "$REPORT_DIR/Cargo.lock" <<'PY'
import hashlib
import sys
from pathlib import Path
print(hashlib.sha256(Path(sys.argv[1]).read_bytes()).hexdigest())
PY
  )
  export EXPECTED_LOCK_SHA
}

new_case() {
  local name=$1
  case_dir="$test_root/$name"
  remote="$case_dir/origin.git"
  seed="$case_dir/seed"
  worker="$case_dir/worker"
  mkdir -p "$case_dir"
  git -c init.defaultBranch=main init --bare --quiet "$remote"
  git -c init.defaultBranch=main init --quiet "$seed"
  git -C "$seed" config user.name 'Security integration fixture'
  git -C "$seed" config user.email 'security-fixture@example.invalid'
  git -C "$seed" config commit.gpgsign false
  write_lock "$seed/Cargo.lock" 1.0.0
  printf '%s\n' '[workspace]' > "$seed/Cargo.toml"
  printf '%s\n' '[advisories]' 'ignore = []' > "$seed/deny.toml"
  printf '%s\n' 'Integration fixture' > "$seed/README.md"
  git -C "$seed" add Cargo.lock Cargo.toml deny.toml README.md
  git -C "$seed" commit --quiet -m 'Create trusted main fixture'
  git -C "$seed" remote add origin "$remote"
  git -C "$seed" push --quiet origin main
  git --git-dir="$remote" symbolic-ref HEAD refs/heads/main
  git clone --quiet "$remote" "$worker"
  git -C "$worker" config commit.gpgsign false
  source_sha=$(git -C "$worker" rev-parse HEAD)
  export REPORT_DIR="$case_dir/report"
  export DEFAULT_BRANCH=main
  export GITHUB_OUTPUT="$case_dir/output"
  mkdir -p "$REPORT_DIR"
  printf '%s\n' "$source_sha" > "$REPORT_DIR/source-sha"
  printf '%s\n' passed > "$REPORT_DIR/validation"
  write_lock "$REPORT_DIR/Cargo.lock" 1.0.2
  capture_expected_lock_hash
  : > "$GITHUB_OUTPUT"
}

invoke() {
  (cd "$worker" && bash "$publisher") > "$case_dir/publisher.stdout" 2> "$case_dir/publisher.stderr"
}

output_value() {
  awk -F= -v key="$1" '$1 == key {value=$2} END {print value}' "$GITHUB_OUTPUT"
}

assert_output() {
  local actual
  actual=$(output_value "$1")
  [ "$actual" = "$2" ] || fail "expected output $1=$2, got '$actual'"
}

remote_sha() {
  git --git-dir="$remote" rev-parse "refs/heads/$1"
}

assert_no_fix_branch() {
  if git --git-dir="$remote" show-ref --verify --quiet "refs/heads/$fix_branch"; then
    fail 'unexpected remediation branch was published'
  fi
}

pass_case() {
  tests=$((tests + 1))
  echo "ok $tests - $1"
}

new_case validated-lockfile-only
printf '%s\n' 'Unrelated untracked file' > "$worker/local-note.txt"
invoke || fail 'a validated candidate should publish'
published=$(remote_sha "$fix_branch")
[ "$(remote_sha main)" = "$source_sha" ] || fail 'publication changed main'
changed_files=$(git --git-dir="$remote" diff-tree --no-commit-id --name-only -r "$published")
[ "$changed_files" = Cargo.lock ] || fail "published unrelated files: $changed_files"
git --git-dir="$remote" show "$published:Cargo.lock" | cmp -s - "$REPORT_DIR/Cargo.lock" || fail 'published lockfile differs from validated artifact'
[ "$(git --git-dir="$remote" show -s --format='%ae' "$published")" = "$bot_email" ] || fail 'fix commit must identify the bot'
[ -f "$worker/local-note.txt" ] || fail 'publication removed unrelated local data'
assert_output validation passed
assert_output changed true
assert_output pushed true
assert_output updated true
assert_output verified_sha "$published"
pass_case 'publishes the exact validated lockfile and excludes unrelated files'

for integrity in missing malformed tampered; do
  new_case "artifact-hash-$integrity"
  case "$integrity" in
    missing) unset EXPECTED_LOCK_SHA ;;
    malformed) export EXPECTED_LOCK_SHA=not-a-sha256 ;;
    tampered) write_lock "$REPORT_DIR/Cargo.lock" 1.0.99 ;;
  esac
  if invoke; then
    fail "a $integrity lockfile integrity check should prevent publication"
  fi
  assert_no_fix_branch
  [ "$(remote_sha main)" = "$source_sha" ] || fail 'an integrity failure changed main'
  cmp -s "$worker/Cargo.lock" "$seed/Cargo.lock" || fail 'an integrity failure copied the unverified artifact'
  assert_output pushed false
  assert_output updated false
  [ -z "$(output_value verified_sha)" ] || fail 'an integrity failure reported a verified branch'
  pass_case "a $integrity pre-build lockfile hash prevents publication"
done

for dirty in tracked staged; do
  new_case "dirty-$dirty"
  printf '%s\n' 'Unrelated local documentation change' >> "$worker/README.md"
  if [ "$dirty" = staged ]; then
    git -C "$worker" add README.md
  fi
  if invoke; then
    fail "a $dirty unrelated change should be rejected"
  fi
  assert_no_fix_branch
  [ "$(remote_sha main)" = "$source_sha" ] || fail 'dirty checkout publication changed main'
  assert_output pushed false
  assert_output updated false
  pass_case "a $dirty unrelated change prevents publication"
done

for validation in failed not_run; do
  new_case "validation-$validation"
  printf '%s\n' "$validation" > "$REPORT_DIR/validation"
  invoke || fail "validation=$validation should be a successful no-op"
  assert_no_fix_branch
  [ "$(remote_sha main)" = "$source_sha" ] || fail 'non-passing validation changed main'
  cmp -s "$worker/Cargo.lock" "$seed/Cargo.lock" || fail 'non-passing validation copied a candidate'
  assert_output validation "$validation"
  assert_output changed false
  assert_output pushed false
  assert_output updated false
  pass_case "validation=$validation leaves the repository and remote unchanged"
done

new_case identical-candidate
invoke || fail 'initial identical-candidate setup should publish'
first_sha=$(remote_sha "$fix_branch")
git -C "$worker" checkout --quiet --detach "$source_sha"
: > "$GITHUB_OUTPUT"
invoke || fail 'an identical repeated candidate should succeed'
[ "$(remote_sha "$fix_branch")" = "$first_sha" ] || fail 'identical candidate created another commit'
assert_output changed true
assert_output pushed true
assert_output updated false
assert_output verified_sha "$first_sha"
pass_case 'an identical repeat preserves the branch SHA and reports updated=false'

new_case refresh-after-main-advances
invoke || fail 'initial candidate should publish before main advances'
previous_fix_sha=$(remote_sha "$fix_branch")
printf '%s\n' 'Legitimate source change on main after the previous scan' >> "$seed/README.md"
git -C "$seed" add README.md
git -C "$seed" commit --quiet -m 'Update trusted main between scheduled scans'
git -C "$seed" push --quiet origin main
source_sha=$(remote_sha main)
git -C "$worker" fetch --quiet origin main
git -C "$worker" checkout --quiet --detach "$source_sha"
printf '%s\n' "$source_sha" > "$REPORT_DIR/source-sha"
: > "$GITHUB_OUTPUT"
invoke || fail 'a freshly validated candidate should refresh the safe bot branch on newer main'
refreshed_sha=$(remote_sha "$fix_branch")
[ "$refreshed_sha" != "$previous_fix_sha" ] || fail 'refresh kept the stale candidate commit'
[ "$(remote_sha main)" = "$source_sha" ] || fail 'refresh changed main'
[ "$(git --git-dir="$remote" rev-parse "$refreshed_sha^")" = "$source_sha" ] || fail 'refreshed candidate is not based on newly validated main'
changed_files=$(git --git-dir="$remote" diff --name-only "$source_sha" "$refreshed_sha")
[ "$changed_files" = Cargo.lock ] || fail "refreshed candidate includes unrelated changes: $changed_files"
git --git-dir="$remote" show "$refreshed_sha:Cargo.lock" | cmp -s - "$REPORT_DIR/Cargo.lock" || fail 'refresh changed the validated lockfile'
git --git-dir="$remote" show "$refreshed_sha:README.md" | cmp -s - "$seed/README.md" || fail 'refresh lost the legitimate main change'
assert_output pushed true
assert_output updated true
assert_output verified_sha "$refreshed_sha"
pass_case 'a safe previous fix is recreated on freshly validated main with only a lockfile delta'

new_case unchanged-lockfile
cp "$seed/Cargo.lock" "$REPORT_DIR/Cargo.lock"
capture_expected_lock_hash
invoke || fail 'an unchanged validated lockfile should be a successful no-op'
assert_no_fix_branch
assert_output changed false
assert_output pushed false
assert_output updated false
pass_case 'an unchanged validated lockfile does not publish a branch'

new_case advanced-main
printf '%s\n' 'New main change during validation' >> "$seed/README.md"
git -C "$seed" add README.md
git -C "$seed" commit --quiet -m 'Advance main while candidate validates'
git -C "$seed" push --quiet origin main
advanced_sha=$(remote_sha main)
if invoke; then
  fail 'a candidate validated against stale main should be rejected'
fi
assert_no_fix_branch
[ "$(remote_sha main)" = "$advanced_sha" ] || fail 'rejected stale candidate changed main'
assert_output pushed false
assert_output updated false
pass_case 'main advancing during validation prevents publication'

new_case human-owned-reserved-branch
git -C "$seed" checkout --quiet -b "$fix_branch"
write_lock "$seed/Cargo.lock" 1.0.1
git -C "$seed" add Cargo.lock
git -C "$seed" commit --quiet -m 'Human-owned remediation work'
git -C "$seed" push --quiet origin "$fix_branch"
human_sha=$(remote_sha "$fix_branch")
if invoke; then
  fail 'a non-bot commit on the reserved branch should be rejected'
fi
[ "$(remote_sha "$fix_branch")" = "$human_sha" ] || fail 'publisher replaced human work'
assert_output pushed false
assert_output updated false
pass_case 'a human commit on the reserved branch is preserved'

new_case bot-branch-unrelated-change
git -C "$seed" checkout --quiet -b "$fix_branch"
git -C "$seed" config user.name 'github-actions[bot]'
git -C "$seed" config user.email "$bot_email"
cp "$REPORT_DIR/Cargo.lock" "$seed/Cargo.lock"
printf '%s\n' 'Unreviewed source changes with the bot email' >> "$seed/README.md"
git -C "$seed" add Cargo.lock README.md
git -C "$seed" commit --quiet -m 'Spoofed bot candidate includes unrelated changes'
git -C "$seed" push --quiet origin "$fix_branch"
spoofed_sha=$(remote_sha "$fix_branch")
if invoke; then
  fail 'matching lockfile and bot email must not validate unrelated branch changes'
fi
[ "$(remote_sha "$fix_branch")" = "$spoofed_sha" ] || fail 'publisher changed an unverified reserved branch'
assert_output pushed false
assert_output updated false
[ -z "$(output_value verified_sha)" ] || fail 'unverified reserved branch was reported as verified'
pass_case 'a bot-email branch with unrelated changes cannot take the identical-lockfile fast path'

new_case concurrent-remote-update
git -C "$seed" checkout --quiet -b "$fix_branch"
git -C "$seed" config user.name 'github-actions[bot]'
git -C "$seed" config user.email "$bot_email"
write_lock "$seed/Cargo.lock" 1.0.1
git -C "$seed" add Cargo.lock
git -C "$seed" commit --quiet -m 'Previous bot candidate'
git -C "$seed" push --quiet origin "$fix_branch"
previous_sha=$(remote_sha "$fix_branch")
write_lock "$seed/Cargo.lock" 1.0.3
git -C "$seed" add Cargo.lock
git -C "$seed" commit --quiet -m 'Concurrent remote work'
race_sha=$(git -C "$seed" rev-parse HEAD)
# Make the object available to the bare remote without moving the guarded ref.
git -C "$seed" push --quiet origin HEAD:refs/heads/race-object
mkdir -p "$case_dir/bin"
cat > "$case_dir/bin/git" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [ "${1:-}" = push ]; then
  "$SECURITY_TEST_REAL_GIT" --git-dir="$SECURITY_TEST_REMOTE" update-ref \
    refs/heads/codex/security-advisories "$SECURITY_TEST_RACE_SHA" "$SECURITY_TEST_PREVIOUS_SHA"
fi
exec "$SECURITY_TEST_REAL_GIT" "$@"
EOF
chmod +x "$case_dir/bin/git"
if PATH="$case_dir/bin:$PATH" SECURITY_TEST_REAL_GIT="$real_git" \
    SECURITY_TEST_REMOTE="$remote" SECURITY_TEST_RACE_SHA="$race_sha" \
    SECURITY_TEST_PREVIOUS_SHA="$previous_sha" invoke; then
  fail 'a concurrent remote update should fail the force-with-lease'
fi
[ "$(remote_sha "$fix_branch")" = "$race_sha" ] || fail 'force-with-lease overwrote concurrent remote work'
assert_output pushed false
assert_output updated false
pass_case 'force-with-lease preserves a concurrent change after branch inspection'

echo "validated $tests local Git publication scenarios"
