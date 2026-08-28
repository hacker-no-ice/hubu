#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
installer="${root_dir}/scripts/install-from-source.sh"
test_dir="$(mktemp -d "${TMPDIR:-/tmp}/hubu-source-install-test.XXXXXX")"
trap 'rm -rf "${test_dir}"' EXIT

expected_commit="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
different_commit="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"

fail_test() {
  echo "source installer test failed: $*" >&2
  exit 1
}

make_stubs() {
  local case_dir="$1"
  local stub_dir="${case_dir}/stubs"
  mkdir -p "${stub_dir}"

  cat >"${stub_dir}/git" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "-C" ]]; then
  shift 2
fi
case "${1:-}" in
  rev-parse)
    case "${2:-}" in
      --show-toplevel) printf '%s\n' "${STUB_CHECKOUT}" ;;
      HEAD) printf '%s\n' "${STUB_HEAD}" ;;
      refs/tags/*) printf '%s\n' "${STUB_TAG_COMMIT:-${STUB_HEAD}}" ;;
      *) exit 2 ;;
    esac
    ;;
  symbolic-ref)
    if [[ "${STUB_ATTACHED:-0}" == "1" ]]; then
      printf '%s\n' refs/heads/main
    else
      exit 1
    fi
    ;;
  tag)
    [[ "${2:-}" == "--points-at" && "${3:-}" == "HEAD" ]]
    printf '%b\n' "${STUB_TAGS:-v1.2.3}"
    ;;
  status)
    if [[ "${STUB_DIRTY:-0}" == "1" ]]; then
      printf '%s\n' '?? local-change'
    fi
    ;;
  *)
    exit 2
    ;;
esac
EOF

  cat >"${stub_dir}/xcode-select" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "${1:-}" == "-p" ]]
if [[ "${STUB_XCODE_FAIL:-0}" == "1" ]]; then
  exit 1
fi
printf '%s\n' /Library/Developer/CommandLineTools
EOF

  cat >"${stub_dir}/protoc" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "${1:-}" == "--version" ]]
if [[ "${STUB_PROTOC_FAIL:-0}" == "1" ]]; then
  exit 1
fi
printf '%s\n' 'libprotoc 35.1'
EOF

  cat >"${stub_dir}/remove" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
target=""
for argument in "$@"; do
  target="${argument}"
done
if [[ -n "${STUB_FAIL_REMOVE_PATH:-}" && "${target}" == "${STUB_FAIL_REMOVE_PATH}" ]]; then
  /bin/rm "$@"
  exit 91
fi
exec /bin/rm "$@"
EOF

  cat >"${stub_dir}/rustup" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "${1:-}" == "run" ]]
toolchain="${2:-}"
[[ "${toolchain}" == "1.94.1" ]]
[[ "${3:-}" == "cargo" ]]
if [[ "${4:-}" == "--version" ]]; then
  if [[ "${STUB_TOOLCHAIN_FAIL:-0}" == "1" ]]; then
    exit 1
  fi
  printf '%s\n' 'cargo 1.94.1 (stub)'
  exit 0
fi
[[ "${4:-}" == "build" ]]
shift 4
printf 'BUILD' >>"${STUB_LOG}"
for argument in "$@"; do
  printf '|%s' "${argument}" >>"${STUB_LOG}"
done
printf '\nMETA|%s|%s|%s|%s|%s\n' \
  "${HUBU_PRODUCT_VERSION}" \
  "${HUBU_SOURCE_COMMIT}" \
  "${GONGBU_PRODUCT_VERSION}" \
  "${GONGBU_SOURCE_COMMIT}" \
  "${GONGBU_BUILD_ID}" >>"${STUB_LOG}"

target_dir=""
while [[ "$#" -gt 0 ]]; do
  if [[ "$1" == "--target-dir" ]]; then
    target_dir="$2"
    break
  fi
  shift
done
[[ -n "${target_dir}" ]]
mkdir -p "${target_dir}/release"
for binary in hubu hubu-server hubu-unified-mcp gongbu-server; do
  binary_commit="${HUBU_SOURCE_COMMIT}"
  binary_version="${HUBU_PRODUCT_VERSION}"
  binary_build_id="not-applicable"
  if [[ "${binary}" == "gongbu-server" ]]; then
    binary_commit="${GONGBU_SOURCE_COMMIT}"
    binary_version="${GONGBU_PRODUCT_VERSION}"
    binary_build_id="${GONGBU_BUILD_ID}"
  fi
  if [[ "${STUB_BAD_METADATA:-0}" == "1" && "${binary}" == "gongbu-server" ]]; then
    binary_commit="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
  fi
  if [[ "${STUB_BAD_BUILD_ID:-0}" == "1" && "${binary}" == "gongbu-server" ]]; then
    binary_build_id="wrong-build-id"
  fi
  cat >"${target_dir}/release/${binary}" <<SCRIPT
#!/usr/bin/env bash
if [[ "\${1:-}" != "--version" ]]; then
  exit 2
fi
if [[ -n "\${STUB_FAIL_INSTALLED_PATH:-}" && "\${STUB_FAIL_INSTALLED_PATH}" == "\$0" ]]; then
  exit 42
fi
cat <<JSON
{
  "product_version": "${binary_version}",
  "source_commit": "${binary_commit}",
  "build_id": "${binary_build_id}",
  "executor_contract": "hubu-spend-executor-v4.3"
}
JSON
SCRIPT
  chmod +x "${target_dir}/release/${binary}"
done
if [[ "${STUB_CREATE_DEV_BINARIES:-0}" == "1" ]]; then
  touch "${target_dir}/release/hubu-bench" "${target_dir}/release/gongbu-sandbox"
fi
EOF

  chmod +x "${stub_dir}/git" "${stub_dir}/xcode-select" \
    "${stub_dir}/protoc" "${stub_dir}/remove" "${stub_dir}/rustup"
}

make_case() {
  local name="$1"
  local case_dir="${test_dir}/${name}"
  mkdir -p "${case_dir}/checkout"
  cat >"${case_dir}/checkout/rust-toolchain.toml" <<'EOF'
[toolchain]
channel = "1.94.1"
profile = "minimal"
EOF
  : >"${case_dir}/log"
  make_stubs "${case_dir}"
  printf '%s\n' "${case_dir}"
}

run_installer() {
  local case_dir="$1"
  shift
  env \
    PATH="${case_dir}/stubs:${PATH}" \
    HUBU_SOURCE_INSTALL_OS_OVERRIDE=Darwin \
    HUBU_SOURCE_INSTALL_CHECKOUT_ROOT="${case_dir}/checkout" \
    HUBU_SOURCE_INSTALL_TMPDIR="${case_dir}" \
    HUBU_SOURCE_INSTALL_REMOVE="${case_dir}/stubs/remove" \
    STUB_CHECKOUT="${case_dir}/checkout" \
    STUB_HEAD="${STUB_HEAD:-${expected_commit}}" \
    STUB_TAG_COMMIT="${STUB_TAG_COMMIT:-${expected_commit}}" \
    STUB_TAGS="${STUB_TAGS:-main-${expected_commit}\nv1.2.3-rc.2\nv1.2.3}" \
    STUB_LOG="${case_dir}/log" \
    STUB_ATTACHED="${STUB_ATTACHED:-0}" \
    STUB_DIRTY="${STUB_DIRTY:-0}" \
    STUB_XCODE_FAIL="${STUB_XCODE_FAIL:-0}" \
    STUB_PROTOC_FAIL="${STUB_PROTOC_FAIL:-0}" \
    STUB_TOOLCHAIN_FAIL="${STUB_TOOLCHAIN_FAIL:-0}" \
    STUB_BAD_METADATA="${STUB_BAD_METADATA:-0}" \
    STUB_BAD_BUILD_ID="${STUB_BAD_BUILD_ID:-0}" \
    STUB_CREATE_DEV_BINARIES="${STUB_CREATE_DEV_BINARIES:-0}" \
    STUB_FAIL_INSTALLED_PATH="${STUB_FAIL_INSTALLED_PATH:-}" \
    STUB_FAIL_REMOVE_PATH="${STUB_FAIL_REMOVE_PATH:-}" \
    HOME="${STUB_HOME:-${HOME}}" \
    "${installer}" "$@"
}

assert_failure() {
  local expected_message="$1"
  local output_file="$2"
  shift 2
  if "$@" >"${output_file}" 2>&1; then
    fail_test "command unexpectedly succeeded; expected ${expected_message}"
  fi
  grep -F "${expected_message}" "${output_file}" >/dev/null || {
    cat "${output_file}" >&2
    fail_test "missing failure message: ${expected_message}"
  }
}

success_case="$(make_case success)"
STUB_CREATE_DEV_BINARIES=1
run_installer \
  "${success_case}" \
  --expected-commit "${expected_commit}" \
  --prefix "${success_case}/prefix" \
  >"${success_case}/output" 2>&1
unset STUB_CREATE_DEV_BINARIES
for binary in hubu hubu-server hubu-unified-mcp gongbu-server; do
  test -x "${success_case}/prefix/bin/${binary}" || fail_test "${binary} was not installed"
  "${success_case}/prefix/bin/${binary}" --version | \
    grep -F "\"source_commit\": \"${expected_commit}\"" >/dev/null
done
test ! -e "${success_case}/prefix/bin/hubu-bench"
test ! -e "${success_case}/prefix/bin/gongbu-sandbox"
[[ "$(grep -c '^BUILD|' "${success_case}/log")" -eq 1 ]] || fail_test "expected one Cargo build"
grep -E '^BUILD\|--release\|--locked\|--target-dir\|[^|]+\|--bin\|hubu\|--bin\|hubu-server\|--bin\|hubu-unified-mcp\|--bin\|gongbu-server$' \
  "${success_case}/log" >/dev/null || fail_test "build did not select exactly four production binaries"
grep -F "META|v1.2.3|${expected_commit}|v1.2.3|${expected_commit}|source-aaaaaaaaaaaa" \
  "${success_case}/log" >/dev/null || fail_test "release build metadata was not stamped"
grep -F "Installed Hubu v1.2.3 (${expected_commit})" "${success_case}/output" >/dev/null

rc_case="$(make_case release-candidate)"
STUB_TAGS="main-${expected_commit}\nv1.2.3-rc.2"
run_installer \
  "${rc_case}" \
  --expected-commit "${expected_commit}" \
  --prefix "${rc_case}/prefix" \
  >"${rc_case}/output" 2>&1
unset STUB_TAGS
grep -F "META|v1.2.3-rc.2|${expected_commit}|v1.2.3-rc.2|${expected_commit}|source-aaaaaaaaaaaa" \
  "${rc_case}/log" >/dev/null || fail_test "release-candidate metadata was not stamped"

canary_case="$(make_case canary)"
STUB_TAGS="main-${expected_commit}"
assert_failure \
  "HEAD is not tagged with one exact Hubu version tag" \
  "${canary_case}/output" \
  run_installer "${canary_case}" --expected-commit "${expected_commit}" --prefix "${canary_case}/prefix"
unset STUB_TAGS
[[ ! -s "${canary_case}/log" ]] || fail_test "canary tag reached the build"

default_prefix_case="$(make_case default-prefix)"
STUB_HOME="${default_prefix_case}/home"
run_installer \
  "${default_prefix_case}" \
  --expected-commit "${expected_commit}" \
  >"${default_prefix_case}/output" 2>&1
unset STUB_HOME
for binary in hubu hubu-server hubu-unified-mcp gongbu-server; do
  test -x "${default_prefix_case}/home/.local/bin/${binary}" || \
    fail_test "default prefix did not install ${binary}"
done

mismatch_case="$(make_case mismatch)"
STUB_HEAD="${different_commit}"
assert_failure \
  "does not match expected commit" \
  "${mismatch_case}/output" \
  run_installer "${mismatch_case}" --expected-commit "${expected_commit}" --prefix "${mismatch_case}/prefix"
unset STUB_HEAD
[[ ! -s "${mismatch_case}/log" ]] || fail_test "SHA mismatch reached the build"
[[ ! -e "${mismatch_case}/prefix/bin" ]] || fail_test "SHA mismatch touched the prefix"

branch_case="$(make_case branch)"
STUB_ATTACHED=1
assert_failure \
  "checkout must be detached at the exact release tag" \
  "${branch_case}/output" \
  run_installer "${branch_case}" --expected-commit "${expected_commit}" --prefix "${branch_case}/prefix"
unset STUB_ATTACHED

dirty_case="$(make_case dirty)"
STUB_DIRTY=1
assert_failure \
  "checkout has tracked or untracked changes" \
  "${dirty_case}/output" \
  run_installer "${dirty_case}" --expected-commit "${expected_commit}" --prefix "${dirty_case}/prefix"
unset STUB_DIRTY

prerequisite_case="$(make_case prerequisite)"
STUB_PROTOC_FAIL=1
assert_failure \
  "protoc is installed but unusable" \
  "${prerequisite_case}/output" \
  run_installer "${prerequisite_case}" --expected-commit "${expected_commit}" --prefix "${prerequisite_case}/prefix"
unset STUB_PROTOC_FAIL

toolchain_case="$(make_case toolchain)"
STUB_TOOLCHAIN_FAIL=1
assert_failure \
  "Rust 1.94.1 with Cargo is required" \
  "${toolchain_case}/output" \
  run_installer "${toolchain_case}" --expected-commit "${expected_commit}" --prefix "${toolchain_case}/prefix"
unset STUB_TOOLCHAIN_FAIL
[[ ! -s "${toolchain_case}/log" ]] || fail_test "missing pinned toolchain reached the build"

metadata_case="$(make_case metadata)"
STUB_BAD_METADATA=1
assert_failure \
  "gongbu-server reports the wrong source commit" \
  "${metadata_case}/output" \
  run_installer "${metadata_case}" --expected-commit "${expected_commit}" --prefix "${metadata_case}/prefix"
unset STUB_BAD_METADATA
[[ ! -e "${metadata_case}/prefix/bin" ]] || fail_test "bad staged metadata touched the prefix"

build_id_case="$(make_case build-id)"
STUB_BAD_BUILD_ID=1
assert_failure \
  "gongbu-server reports the wrong build ID" \
  "${build_id_case}/output" \
  run_installer "${build_id_case}" --expected-commit "${expected_commit}" --prefix "${build_id_case}/prefix"
unset STUB_BAD_BUILD_ID
[[ ! -e "${build_id_case}/prefix/bin" ]] || fail_test "bad Gongbu build ID touched the prefix"

remove_failure_case="$(make_case remove-failure)"
mkdir -p "${remove_failure_case}/prefix/bin"
printf '%s\n' 'old-hubu' >"${remove_failure_case}/prefix/bin/hubu"
chmod +x "${remove_failure_case}/prefix/bin/hubu"
STUB_FAIL_REMOVE_PATH="${remove_failure_case}/prefix/bin/hubu"
assert_failure \
  "cannot prepare ${remove_failure_case}/prefix/bin/hubu for replacement" \
  "${remove_failure_case}/output" \
  run_installer "${remove_failure_case}" --expected-commit "${expected_commit}" --prefix "${remove_failure_case}/prefix"
unset STUB_FAIL_REMOVE_PATH
grep -Fx 'old-hubu' "${remove_failure_case}/prefix/bin/hubu" >/dev/null || \
  fail_test "rollback did not restore a binary removed before replacement"
[[ -x "${remove_failure_case}/prefix/bin/hubu" ]] || \
  fail_test "remove-failure rollback did not preserve executable mode"

rollback_case="$(make_case rollback)"
mkdir -p "${rollback_case}/prefix/bin"
printf '%s\n' 'old-hubu' >"${rollback_case}/prefix/bin/hubu"
printf '%s\n' 'old-gongbu-server' >"${rollback_case}/prefix/bin/gongbu-server"
chmod +x "${rollback_case}/prefix/bin/hubu" "${rollback_case}/prefix/bin/gongbu-server"
STUB_FAIL_INSTALLED_PATH="${rollback_case}/prefix/bin/hubu-server"
assert_failure \
  "installed hubu-server --version failed" \
  "${rollback_case}/output" \
  run_installer "${rollback_case}" --expected-commit "${expected_commit}" --prefix "${rollback_case}/prefix"
unset STUB_FAIL_INSTALLED_PATH
grep -Fx 'old-hubu' "${rollback_case}/prefix/bin/hubu" >/dev/null || \
  fail_test "rollback did not restore the previous hubu binary"
[[ -x "${rollback_case}/prefix/bin/hubu" ]] || \
  fail_test "rollback did not preserve the previous hubu executable mode"
grep -Fx 'old-gongbu-server' "${rollback_case}/prefix/bin/gongbu-server" >/dev/null || \
  fail_test "rollback changed an unreplaced Gongbu binary"
[[ ! -e "${rollback_case}/prefix/bin/hubu-server" ]] || \
  fail_test "rollback left a partially installed hubu-server"
[[ ! -e "${rollback_case}/prefix/bin/hubu-unified-mcp" ]] || \
  fail_test "rollback left a partially installed hubu-unified-mcp"
if find "${rollback_case}/prefix/bin" -maxdepth 1 -name '.hubu-source-install.*' | grep -q .; then
  fail_test "rollback left install transaction files behind"
fi

directory_symlink_case="$(make_case directory-symlink)"
mkdir -p "${directory_symlink_case}/prefix/bin" "${directory_symlink_case}/outside"
printf '%s\n' 'outside-sentinel' >"${directory_symlink_case}/outside/sentinel"
ln -s "${directory_symlink_case}/outside" "${directory_symlink_case}/prefix/bin/hubu"
assert_failure \
  "installation target resolves to a directory" \
  "${directory_symlink_case}/output" \
  run_installer "${directory_symlink_case}" --expected-commit "${expected_commit}" --prefix "${directory_symlink_case}/prefix"
[[ -L "${directory_symlink_case}/prefix/bin/hubu" ]] || \
  fail_test "directory-symlink rejection replaced the destination link"
grep -Fx 'outside-sentinel' "${directory_symlink_case}/outside/sentinel" >/dev/null || \
  fail_test "directory-symlink rejection changed the linked directory"
[[ ! -e "${directory_symlink_case}/outside/hubu" ]] || \
  fail_test "directory-symlink rejection installed outside the selected prefix"

if grep -Eq '(^|[[:space:]])(sudo|xattr|spctl)([[:space:]]|$)' "${installer}" || \
   grep -Eq 'curl.*\|.*(sh|bash)' "${installer}"; then
  fail_test "installer contains a privilege, Gatekeeper, or download-pipe workaround"
fi

echo "Source installer tests passed: exact identity, prerequisites, bounded build, build metadata, staging, and rollback-safe installation"
