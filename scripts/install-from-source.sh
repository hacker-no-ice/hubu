#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Build and install one exact Hubu release checkout.

Usage:
  ./scripts/install-from-source.sh --expected-commit FULL_40_SHA [--prefix /absolute/path]

Options:
  --expected-commit SHA  Required lowercase 40-character release commit.
  --prefix PATH          Installation prefix (default: $HOME/.local).
  -h, --help             Show this help.

The checkout must be clean, detached, and point at an exact vMAJOR.MINOR.PATCH
or vMAJOR.MINOR.PATCH-rc.N tag. The installer downloads no tools and never uses
administrator privileges. It installs hubu, hubu-server, hubu-unified-mcp, and
gongbu-server into PREFIX/bin only after building and verifying all four.
EOF
}

fail() {
  echo "Hubu source install failed: $*" >&2
  exit 1
}

require_command() {
  local command_name="$1"
  local description="$2"

  if [[ "${command_name}" == */* ]]; then
    [[ -x "${command_name}" ]] || fail "${description} is required at ${command_name}"
  elif ! command -v "${command_name}" >/dev/null 2>&1; then
    fail "${description} is required but '${command_name}' is not on PATH"
  fi
}

expected_commit=""
prefix=""

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --expected-commit)
      [[ "$#" -ge 2 ]] || fail "--expected-commit requires a value"
      expected_commit="$2"
      shift 2
      ;;
    --prefix)
      [[ "$#" -ge 2 ]] || fail "--prefix requires a value"
      prefix="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "unknown argument '$1'; run with --help for usage"
      ;;
  esac
done

[[ -n "${expected_commit}" ]] || fail "--expected-commit is required"
[[ "${expected_commit}" =~ ^[0-9a-f]{40}$ ]] || \
  fail "--expected-commit must be an exact lowercase 40-character SHA"

if [[ -z "${prefix}" ]]; then
  [[ -n "${HOME:-}" ]] || fail "HOME is unset; pass an absolute --prefix"
  prefix="${HOME}/.local"
fi
[[ "${prefix}" == /* ]] || fail "--prefix must be an absolute path"
[[ "${prefix}" != "/" ]] || fail "refusing to use the filesystem root as --prefix"

script_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
checkout_root="${HUBU_SOURCE_INSTALL_CHECKOUT_ROOT:-${script_root}}"
requested_checkout_root="${checkout_root}"
checkout_root="$(cd "${requested_checkout_root}" 2>/dev/null && pwd -P)" || \
  fail "checkout directory does not exist: ${requested_checkout_root}"

git_command="${HUBU_SOURCE_INSTALL_GIT:-git}"
uname_command="${HUBU_SOURCE_INSTALL_UNAME:-uname}"
xcode_select_command="${HUBU_SOURCE_INSTALL_XCODE_SELECT:-xcode-select}"
rustup_command="${HUBU_SOURCE_INSTALL_RUSTUP:-rustup}"
protoc_command="${HUBU_SOURCE_INSTALL_PROTOC:-protoc}"
install_command="${HUBU_SOURCE_INSTALL_INSTALL:-install}"
copy_command="${HUBU_SOURCE_INSTALL_COPY:-cp}"
remove_command="${HUBU_SOURCE_INSTALL_REMOVE:-rm}"

require_command "${uname_command}" "uname"
host_os="${HUBU_SOURCE_INSTALL_OS_OVERRIDE:-$("${uname_command}" -s)}"
[[ "${host_os}" == "Darwin" ]] || \
  fail "macOS is required; detected ${host_os:-an unknown operating system}"

require_command "${git_command}" "Git"
require_command "${xcode_select_command}" "Xcode Command Line Tools (xcode-select)"
if ! xcode_path="$("${xcode_select_command}" -p 2>/dev/null)" || [[ -z "${xcode_path}" ]]; then
  fail "Xcode Command Line Tools are unavailable; install them before retrying"
fi
require_command "${rustup_command}" "rustup"
require_command "${protoc_command}" "the protobuf compiler (protoc)"
if ! protoc_version="$("${protoc_command}" --version 2>/dev/null)" || [[ -z "${protoc_version}" ]]; then
  fail "protoc is installed but unusable"
fi
require_command "${install_command}" "the install utility"
require_command "${copy_command}" "the copy utility"
require_command "${remove_command}" "the remove utility"

if ! repository_root="$("${git_command}" -C "${checkout_root}" rev-parse --show-toplevel 2>/dev/null)"; then
  fail "${checkout_root} is not a Git checkout"
fi
repository_root="$(cd "${repository_root}" 2>/dev/null && pwd -P)" || \
  fail "Git reported an unreadable repository root"
[[ "${repository_root}" == "${checkout_root}" ]] || \
  fail "installer must run from the Hubu repository root (${repository_root})"

if ! head_commit="$("${git_command}" -C "${checkout_root}" rev-parse HEAD 2>/dev/null)"; then
  fail "cannot resolve checkout HEAD"
fi
[[ "${head_commit}" == "${expected_commit}" ]] || \
  fail "checkout HEAD ${head_commit} does not match expected commit ${expected_commit}"

if "${git_command}" -C "${checkout_root}" symbolic-ref --quiet HEAD >/dev/null 2>&1; then
  fail "checkout must be detached at the exact release tag, not on a branch"
fi

if ! tags_at_head="$("${git_command}" -C "${checkout_root}" tag --points-at HEAD 2>/dev/null)"; then
  fail "cannot inspect tags at checkout HEAD"
fi
stable_tags=()
candidate_tags=()
while IFS= read -r tag; do
  [[ -n "${tag}" ]] || continue
  if [[ "${tag}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    stable_tags+=("${tag}")
  elif [[ "${tag}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+-rc\.[1-9][0-9]*$ ]]; then
    candidate_tags+=("${tag}")
  fi
done <<<"${tags_at_head}"

if [[ "${#stable_tags[@]}" -eq 1 ]]; then
  release_tag="${stable_tags[0]}"
elif [[ "${#stable_tags[@]}" -gt 1 ]]; then
  fail "multiple stable release tags point at HEAD: ${stable_tags[*]}"
elif [[ "${#candidate_tags[@]}" -eq 1 ]]; then
  release_tag="${candidate_tags[0]}"
elif [[ "${#candidate_tags[@]}" -gt 1 ]]; then
  fail "multiple release-candidate tags point at HEAD: ${candidate_tags[*]}"
else
  fail "HEAD is not tagged with one exact Hubu version tag"
fi

if ! tag_commit="$("${git_command}" -C "${checkout_root}" rev-parse "refs/tags/${release_tag}^{commit}" 2>/dev/null)"; then
  fail "cannot resolve release tag ${release_tag}"
fi
[[ "${tag_commit}" == "${expected_commit}" ]] || \
  fail "release tag ${release_tag} resolves to ${tag_commit}, not ${expected_commit}"

if ! checkout_status="$("${git_command}" -C "${checkout_root}" status --porcelain=v1 --untracked-files=all 2>/dev/null)"; then
  fail "cannot inspect checkout status"
fi
[[ -z "${checkout_status}" ]] || \
  fail "checkout has tracked or untracked changes; use a clean exact release checkout"

toolchain_file="${checkout_root}/rust-toolchain.toml"
[[ -f "${toolchain_file}" ]] || fail "release checkout is missing rust-toolchain.toml"
toolchain="$(sed -n 's/^[[:space:]]*channel[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "${toolchain_file}")"
[[ "${toolchain}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || \
  fail "rust-toolchain.toml must pin one exact numeric Rust toolchain"
if ! "${rustup_command}" run "${toolchain}" cargo --version >/dev/null 2>&1; then
  fail "Rust ${toolchain} with Cargo is required; install that exact toolchain with rustup before retrying"
fi

production_binaries=(
  hubu
  hubu-server
  hubu-unified-mcp
  gongbu-server
)
development_binaries=(
  hubu-bench
  gongbu-sandbox
)

work_parent="${HUBU_SOURCE_INSTALL_TMPDIR:-${TMPDIR:-/tmp}}"
[[ -d "${work_parent}" ]] || fail "temporary directory parent does not exist: ${work_parent}"
work_dir="$(mktemp -d "${work_parent%/}/hubu-source-install.XXXXXX")" || \
  fail "cannot create a temporary build directory under ${work_parent}"
transaction_dir=""
transaction_active=0
transaction_replaced=0
transaction_had_existing=()

rollback_install() {
  local rollback_failed=0
  local index binary destination backup

  index=$((transaction_replaced - 1))
  while [[ "${index}" -ge 0 ]]; do
    binary="${production_binaries[${index}]}"
    destination="${destination_bin}/${binary}"
    backup="${transaction_dir}/backup/${binary}"
    if [[ "${transaction_had_existing[${index}]:-0}" == "1" ]]; then
      if ! mv -f "${backup}" "${destination}"; then
        echo "Hubu source install rollback failed while restoring ${destination}" >&2
        rollback_failed=1
      fi
    elif ! rm -f "${destination}"; then
      echo "Hubu source install rollback failed while removing ${destination}" >&2
      rollback_failed=1
    fi
    index=$((index - 1))
  done

  transaction_active=0
  if [[ "${rollback_failed}" -eq 0 ]]; then
    rm -rf "${transaction_dir}"
  else
    echo "Recovery copies remain in ${transaction_dir}" >&2
  fi
  return "${rollback_failed}"
}

cleanup() {
  local exit_status=$?

  trap - EXIT
  if [[ "${transaction_active}" -eq 1 ]]; then
    rollback_install || true
  fi
  rm -rf "${work_dir}"
  exit "${exit_status}"
}
trap cleanup EXIT

target_dir="${work_dir}/target"
stage_bin="${work_dir}/stage/bin"
mkdir -p "${stage_bin}"

echo "Building Hubu ${release_tag} at ${expected_commit} with Rust ${toolchain}..."
if ! (
  cd "${checkout_root}"
  HUBU_PRODUCT_VERSION="${release_tag}" \
  HUBU_SOURCE_COMMIT="${expected_commit}" \
  GONGBU_PRODUCT_VERSION="${release_tag}" \
  GONGBU_SOURCE_COMMIT="${expected_commit}" \
  GONGBU_BUILD_ID="source-${expected_commit:0:12}" \
    "${rustup_command}" run "${toolchain}" cargo build \
      --release \
      --locked \
      --target-dir "${target_dir}" \
      --bin hubu \
      --bin hubu-server \
      --bin hubu-unified-mcp \
      --bin gongbu-server
); then
  fail "the locked four-binary release build failed"
fi

verify_binary() {
  local binary_path="$1"
  local binary_name="$2"
  local verification_label="$3"
  local version_output compact_output

  [[ -s "${binary_path}" && -x "${binary_path}" ]] || \
    fail "${verification_label} is missing, empty, or not executable at ${binary_path}"
  if ! version_output="$("${binary_path}" --version 2>/dev/null)"; then
    fail "${verification_label} --version failed"
  fi
  compact_output="$(printf '%s' "${version_output}" | tr -d '[:space:]')"
  [[ "${compact_output}" == *"\"product_version\":\"${release_tag}\""* ]] || \
    fail "${verification_label} reports the wrong product version"
  [[ "${compact_output}" == *"\"source_commit\":\"${expected_commit}\""* ]] || \
    fail "${verification_label} reports the wrong source commit"
  [[ "${compact_output}" == *"\"hubu-spend-executor-v4.3\""* ]] || \
    fail "${verification_label} reports the wrong executor contract"
  if [[ "${binary_name}" == "gongbu-server" ]]; then
    [[ "${compact_output}" == *"\"build_id\":\"source-${expected_commit:0:12}\""* ]] || \
      fail "${verification_label} reports the wrong build ID"
  fi
}

for binary in "${production_binaries[@]}"; do
  built_binary="${target_dir}/release/${binary}"
  verify_binary "${built_binary}" "${binary}" "${binary}"
  "${install_command}" -m 0755 "${built_binary}" "${stage_bin}/${binary}" || \
    fail "cannot stage ${binary}"
done
for binary in "${development_binaries[@]}"; do
  [[ ! -e "${stage_bin}/${binary}" ]] || fail "development binary ${binary} entered the install stage"
done

shopt -s nullglob
staged_entries=("${stage_bin}"/*)
shopt -u nullglob
[[ "${#staged_entries[@]}" -eq "${#production_binaries[@]}" ]] || \
  fail "install stage does not contain exactly four production binaries"
for binary in "${production_binaries[@]}"; do
  verify_binary "${stage_bin}/${binary}" "${binary}" "staged ${binary}"
done

destination_bin="${prefix%/}/bin"
mkdir -p "${destination_bin}" || fail "cannot create installation directory ${destination_bin}"
[[ -d "${destination_bin}" ]] || fail "installation destination is not a directory: ${destination_bin}"
for binary in "${production_binaries[@]}"; do
  destination="${destination_bin}/${binary}"
  if [[ -d "${destination}" ]]; then
    fail "installation target resolves to a directory, not a binary: ${destination}"
  fi
done

transaction_dir="$(mktemp -d "${destination_bin}/.hubu-source-install.XXXXXX")" || \
  fail "cannot create a same-filesystem install transaction in ${destination_bin}"
transaction_active=1
mkdir -p "${transaction_dir}/backup" "${transaction_dir}/new" || \
  fail "cannot prepare the install transaction in ${destination_bin}"

index=0
for binary in "${production_binaries[@]}"; do
  destination="${destination_bin}/${binary}"
  transaction_had_existing[${index}]=0
  if [[ -e "${destination}" || -L "${destination}" ]]; then
    "${copy_command}" -pP "${destination}" "${transaction_dir}/backup/${binary}" || \
      fail "cannot back up existing ${binary} before installation"
    transaction_had_existing[${index}]=1
  fi
  "${install_command}" -m 0755 "${stage_bin}/${binary}" "${transaction_dir}/new/${binary}" || \
    fail "cannot prepare ${binary} for installation"
  verify_binary "${transaction_dir}/new/${binary}" "${binary}" "prepared ${binary}"
  index=$((index + 1))
done

index=0
for binary in "${production_binaries[@]}"; do
  destination="${destination_bin}/${binary}"
  if [[ -e "${destination}" || -L "${destination}" ]]; then
    if [[ -d "${destination}" ]]; then
      fail "installation target resolves to a directory, not a binary: ${destination}"
    fi
    transaction_replaced=$((index + 1))
    if ! "${remove_command}" -f "${destination}"; then
      fail "cannot prepare ${destination} for replacement"
    fi
  else
    transaction_replaced=$((index + 1))
  fi
  if ! mv -f "${transaction_dir}/new/${binary}" "${destination}"; then
    fail "cannot replace ${binary} in ${destination_bin}"
  fi
  verify_binary "${destination}" "${binary}" "installed ${binary}"
  index=$((index + 1))
done

transaction_active=0
if ! rm -rf "${transaction_dir}"; then
  echo "Warning: installed binaries are valid, but cleanup failed for ${transaction_dir}" >&2
fi

echo "Installed Hubu ${release_tag} (${expected_commit}) into ${destination_bin}"
echo "No development binaries, administrator commands, or Gatekeeper overrides were used."
