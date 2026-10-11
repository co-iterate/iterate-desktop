#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
RELEASE_TAG="${ITERATE_RELEASE_TAG:-}"

PASS_COUNT=0
WARN_COUNT=0
FAIL_COUNT=0

pass() {
  PASS_COUNT=$((PASS_COUNT + 1))
  printf '[pass] %s\n' "$1"
}

warn() {
  WARN_COUNT=$((WARN_COUNT + 1))
  printf '[warn] %s\n' "$1"
}

fail() {
  FAIL_COUNT=$((FAIL_COUNT + 1))
  printf '[fail] %s\n' "$1"
}

require_file() {
  local path="$1"
  local label="$2"
  if [[ -e "${REPO_ROOT}/${path}" ]]; then
    pass "${label}: ${path}"
  else
    fail "${label} missing: ${path}"
  fi
}

require_package_script() {
  local script_name="$1"
  if node -e "const p=require('./package.json'); process.exit(p.scripts && p.scripts['${script_name}'] ? 0 : 1)" >/dev/null 2>&1; then
    pass "package script exists: ${script_name}"
  else
    fail "package script missing: ${script_name}"
  fi
}

optional_command() {
  local command_name="$1"
  if command -v "${command_name}" >/dev/null 2>&1; then
    pass "command available: ${command_name}"
  else
    warn "command unavailable: ${command_name}"
  fi
}

check_workflows() {
  require_file ".github/workflows/macos-sign-notarize.yml" "macOS signing workflow"
  require_file ".github/workflows/windows-package.yml" "Windows package workflow"
  require_file ".github/workflows/release.yml" "multi-platform release workflow"
  require_file ".github/workflows/manual-release.yml" "manual release workflow"
}

check_package_scripts() {
  require_package_script "build"
  require_package_script "tauri:build"
  require_package_script "delivery:macos"
  require_package_script "notarize:macos"
  require_package_script "release:sign-assets"
  require_package_script "test:codex-room"
  require_package_script "test:release-signing"
  require_package_script "test:security-regression"
  require_package_script "version:check"
}

check_release_inputs() {
  require_file "scripts/prepare-macos-delivery.sh" "macOS delivery script"
  require_file "scripts/notarize-macos-app.sh" "macOS notarization script"
  require_file "scripts/release-sign-assets.mjs" "release integrity signing script"
  require_file "scripts/iterate-security-regression-check.sh" "security regression script"
  require_file "docs/release/release-integrity-runbook-2026-06-27.md" "release integrity runbook"
  require_file "docs/release/security-closure-2026-06-27.md" "security closure runbook"
  require_file "release-package/windows/Install iterate.bat" "Windows install helper"
  require_file "release-package/windows/Start iterate.bat" "Windows start helper"
  require_file "docs/iterate_安装指南.md" "installation guide"
}

check_release_public_key_embedding_contract() {
  if grep -Fq 'option_env!("ITERATE_RELEASE_PUBLIC_KEY_B64")' "${REPO_ROOT}/src/rust/ui/updater.rs"; then
    pass "Windows/Linux updater embeds release public key at build time"
  else
    fail "Windows/Linux updater must use option_env!(\"ITERATE_RELEASE_PUBLIC_KEY_B64\")"
  fi

  if grep -Fq "ITERATE_RELEASE_PUBLIC_KEY_B64" "${REPO_ROOT}/.github/workflows/windows-package.yml"; then
    pass "Windows package workflow injects release public key"
  else
    fail "Windows package workflow missing ITERATE_RELEASE_PUBLIC_KEY_B64"
  fi

  if grep -Fq "ITERATE_RELEASE_PUBLIC_KEY_B64" "${REPO_ROOT}/.github/workflows/release.yml"; then
    pass "multi-platform release workflow injects release public key"
  else
    fail "multi-platform release workflow missing ITERATE_RELEASE_PUBLIC_KEY_B64"
  fi
}

check_script_syntax() {
  local scripts=(
    "scripts/prepare-macos-delivery.sh"
    "scripts/notarize-macos-app.sh"
    "scripts/iterate-public-stability-check.sh"
    "scripts/iterate-auto-transport-stability-check.sh"
    "scripts/iterate-security-regression-check.sh"
    "scripts/sync-version.sh"
  )
  local script
  for script in "${scripts[@]}"; do
    if bash -n "${REPO_ROOT}/${script}"; then
      pass "bash syntax ok: ${script}"
    else
      fail "bash syntax failed: ${script}"
    fi
  done
}

check_local_macos_bundle_if_present() {
  local app_path="${REPO_ROOT}/target/release/bundle/macos/iterate.app"
  local macos_dir="${app_path}/Contents/MacOS"

  if [[ ! -d "${app_path}" ]]; then
    warn "local macOS app bundle absent; run pnpm tauri:build before bundle checks"
    return
  fi

  if [[ -f "${macos_dir}/iterate" ]]; then
    pass "macOS bundle contains iterate binary"
  else
    fail "macOS bundle missing iterate binary"
  fi

  if [[ -f "${macos_dir}/mcp-server" ]]; then
    pass "macOS bundle contains mcp-server binary"
  else
    fail "macOS bundle missing mcp-server binary"
  fi

  local unexpected
  unexpected="$(find "${macos_dir}" -maxdepth 1 -type f ! -name iterate ! -name mcp-server -print 2>/dev/null || true)"
  if [[ -z "${unexpected}" ]]; then
    pass "macOS bundle has no unexpected executable payloads"
  else
    fail "macOS bundle has unexpected executable payloads: ${unexpected}"
  fi

  if codesign --verify --deep --strict "${app_path}" >/dev/null 2>&1; then
    pass "codesign verification ok for local macOS bundle"
  else
    warn "codesign verification unavailable or failed for local macOS bundle"
  fi
}

check_delivery_artifacts_if_present() {
  local delivery_dir="${REPO_ROOT}/target/release/delivery/macos"
  if [[ ! -d "${delivery_dir}" ]]; then
    warn "macOS delivery directory absent; run pnpm delivery:macos before artifact checks"
    return
  fi

  local artifact_count
  artifact_count="$(find "${delivery_dir}" -maxdepth 1 -type f | wc -l | tr -d ' ')"
  if [[ "${artifact_count}" -gt 0 ]]; then
    pass "macOS delivery artifacts present: ${artifact_count}"
  else
    fail "macOS delivery directory is empty"
  fi
}

check_github_release_if_requested() {
  if [[ -z "${RELEASE_TAG}" ]]; then
    warn "ITERATE_RELEASE_TAG not set; skipping GitHub release metadata check"
    return
  fi

  if ! command -v gh >/dev/null 2>&1; then
    fail "gh is required when ITERATE_RELEASE_TAG is set"
    return
  fi

  if gh release view "${RELEASE_TAG}" --json tagName,name,isDraft,isPrerelease,url >/dev/null; then
    pass "GitHub release metadata readable: ${RELEASE_TAG}"
  else
    fail "GitHub release metadata check failed: ${RELEASE_TAG}"
  fi
}

main() {
  cd "${REPO_ROOT}"

  printf 'iterate release readiness check\n'
  printf 'repo=%s\n' "${REPO_ROOT}"
  printf 'release_tag=%s\n' "${RELEASE_TAG:-<not-set>}"
  printf '\n'

  optional_command node
  optional_command pnpm
  optional_command cargo
  optional_command codesign
  optional_command gh
  printf '\n'

  check_package_scripts
  check_workflows
  check_release_inputs
  check_release_public_key_embedding_contract
  check_script_syntax
  check_local_macos_bundle_if_present
  check_delivery_artifacts_if_present
  check_github_release_if_requested

  printf '\n'
  printf 'summary: pass=%s warn=%s fail=%s\n' "${PASS_COUNT}" "${WARN_COUNT}" "${FAIL_COUNT}"

  if [[ "${FAIL_COUNT}" -gt 0 ]]; then
    exit 1
  fi
}

main "$@"
