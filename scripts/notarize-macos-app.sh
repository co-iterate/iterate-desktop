#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
source "${SCRIPT_DIR}/macos-release-identity.sh"
export CUNZHI_MACOS_ALLOW_DIRTY_SOURCE=0

resolve_repo_path() {
  case "$1" in
    /*) printf '%s\n' "$1" ;;
    *) printf '%s/%s\n' "${REPO_ROOT}" "$1" ;;
  esac
}

if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  CARGO_TARGET_DIR_RESOLVED="$(resolve_repo_path "${CARGO_TARGET_DIR}")"
else
  CARGO_TARGET_DIR_RESOLVED="$(cargo metadata --manifest-path "${REPO_ROOT}/Cargo.toml" --format-version 1 --no-deps \
    | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p' | head -n 1)"
  [[ -n "${CARGO_TARGET_DIR_RESOLVED}" ]] || { echo 'Cannot resolve Cargo target directory' >&2; exit 1; }
fi
APP_PATH="${CARGO_TARGET_DIR_RESOLVED}/release/bundle/macos/iterate.app"
DELIVERY_DIR="$(resolve_repo_path "${CUNZHI_MACOS_DELIVERY_DIR:-${CARGO_TARGET_DIR_RESOLVED}/release/delivery/macos}")"
ZIP_PATH="${DELIVERY_DIR}/iterate.app.zip"
SOURCE_RECEIPT_TOOL="${REPO_ROOT}/scripts/macos-source-receipt.mjs"
INSTALLATION_PATH="${DELIVERY_DIR}/INSTALLATION.md"
PROMPT_PATH="${DELIVERY_DIR}/INSTALL_PROMPT.md"
PROFILE_NAME="${CUNZHI_NOTARY_PROFILE:-cunzhi-notary}"
TEAM_ID="${APPLE_TEAM_ID:-${CUNZHI_NOTARY_TEAM_ID:-UM3Z9G5DNH}}"
SIGN_IDENTITY="${CUNZHI_MACOS_SIGN_IDENTITY:-}"

detect_sign_identity() {
  resolve_macos_release_sign_identity "${SIGN_IDENTITY}"
}

have_keychain_profile() {
  xcrun notarytool history \
    --keychain-profile "${PROFILE_NAME}" \
    --output-format json \
    --no-progress >/dev/null 2>&1
}

submit_with_profile() {
  local artifact_path="$1"

  xcrun notarytool submit "${artifact_path}" \
    --keychain-profile "${PROFILE_NAME}" \
    --wait \
    --no-progress
}

submit_with_env_credentials() {
  local artifact_path="$1"

  xcrun notarytool submit "${artifact_path}" \
    --apple-id "${APPLE_ID}" \
    --password "${APPLE_APP_SPECIFIC_PASSWORD}" \
    --team-id "${TEAM_ID}" \
    --wait \
    --no-progress
}

print_setup_help() {
  cat <<EOF
No usable notarization credentials found.

One-time setup:
  xcrun notarytool store-credentials "${PROFILE_NAME}" --apple-id "<apple-id>" --team-id "${TEAM_ID}" --password "<app-specific-password>"

Then rerun:
  pnpm notarize:macos

Alternatively, export all of these before rerunning:
  APPLE_ID
  APPLE_APP_SPECIFIC_PASSWORD
  APPLE_TEAM_ID
EOF
}

submit_notarization() {
  local artifact_path="$1"

  if have_keychain_profile; then
    printf 'Submitting %s with keychain profile: %s\n' "${artifact_path}" "${PROFILE_NAME}"
    submit_with_profile "${artifact_path}"
  elif [[ -n "${APPLE_ID:-}" && -n "${APPLE_APP_SPECIFIC_PASSWORD:-}" && -n "${TEAM_ID:-}" ]]; then
    printf 'Submitting %s with APPLE_ID / app-specific password credentials.\n' "${artifact_path}"
    submit_with_env_credentials "${artifact_path}"
  else
    print_setup_help
    exit 1
  fi
}

assess_gatekeeper_nonfatal() {
  local label="$1"
  shift

  if "$@"; then
    printf '%s Gatekeeper assessment: accepted\n' "${label}"
  else
    local status=$?
    printf '%s Gatekeeper assessment did not complete successfully (exit %s); continuing after notarization and stapler validation.\n' "${label}" "${status}" >&2
  fi
}

find_delivery_dmg() {
  local dmg_path

  dmg_path="$(find "${DELIVERY_DIR}" -maxdepth 1 -type f -name '*.dmg' | head -n 1)"
  if [[ -z "${dmg_path}" ]]; then
    echo "Missing delivery DMG in ${DELIVERY_DIR}" >&2
    exit 1
  fi

  printf '%s\n' "${dmg_path}"
}

assert_app_entitlement() {
  local entitlement_key="$1"
  local entitlements_file
  local entitlements_dump

  entitlements_file="$(mktemp "${TMPDIR:-/tmp}/iterate-entitlements.XXXXXX.plist")"
  if ! codesign -d --entitlements :- "${APP_PATH}" >"${entitlements_file}" 2>/dev/null; then
    rm -f "${entitlements_file}"
    echo "Failed to read entitlements from ${APP_PATH}" >&2
    exit 1
  fi

  entitlements_dump="$(plutil -p "${entitlements_file}" 2>/dev/null || true)"
  rm -f "${entitlements_file}"

  # 不同 macOS 版本的 plutil -p 把布尔值打印成 true 或 1。
  if ! grep -Eq "\"${entitlement_key//./\\.}\" => (true|1)\$" <<<"${entitlements_dump}"; then
    echo "Missing required app entitlement: ${entitlement_key}" >&2
    echo "${entitlements_dump}" >&2
    exit 1
  fi
}

assert_required_app_entitlements() {
  assert_app_entitlement "com.apple.security.automation.apple-events"
  assert_app_entitlement "com.apple.security.device.audio-input"
}

rebuild_delivery_dmg() {
  local existing_dmg
  local dmg_name
  local output_dmg_path
  local stage_dir

  existing_dmg="$(find "${DELIVERY_DIR}" -maxdepth 1 -type f -name '*.dmg' | head -n 1)"
  if [[ -n "${existing_dmg}" ]]; then
    dmg_name="$(basename "${existing_dmg}")"
  else
    dmg_name="$(basename "$(find "${CARGO_TARGET_DIR_RESOLVED}/release/bundle/dmg" -maxdepth 1 -type f -name '*.dmg' | head -n 1)")"
  fi

  if [[ -z "${dmg_name}" ]]; then
    echo "Missing DMG name to rebuild delivery artifact" >&2
    exit 1
  fi

  output_dmg_path="${DELIVERY_DIR}/${dmg_name}"
  stage_dir="$(mktemp -d "${TMPDIR:-/tmp}/iterate-dmg-stage.XXXXXX")"
  trap 'rm -rf "${stage_dir}"' RETURN

  cp -R "${APP_PATH}" "${stage_dir}/iterate.app"

  if [[ -f "${INSTALLATION_PATH}" ]]; then
    cp "${INSTALLATION_PATH}" "${stage_dir}/INSTALLATION.md"
  fi

  if [[ -f "${PROMPT_PATH}" ]]; then
    cp "${PROMPT_PATH}" "${stage_dir}/INSTALL_PROMPT.md"
  fi

  rm -f "${output_dmg_path}"
  hdiutil create \
    -volname "iterate" \
    -srcfolder "${stage_dir}" \
    -ov \
    -format UDZO \
    "${output_dmg_path}" >/dev/null
  hdiutil verify "${output_dmg_path}" >/dev/null

  trap - RETURN
  rm -rf "${stage_dir}"
}

notarize_delivery_dmg() {
  local dmg_path
  local identity

  dmg_path="$(find_delivery_dmg)"
  identity="$(detect_sign_identity || true)"

  if [[ -z "${identity}" ]]; then
    echo "No Developer ID identity found for DMG signing." >&2
    exit 1
  fi

  printf 'Signing delivery DMG with Developer ID identity: %s\n' "${identity}"
  codesign --force --timestamp --sign "${identity}" "${dmg_path}"
  codesign --verify --strict --verbose=2 -R "=${MACOS_RELEASE_CODE_REQUIREMENT}" "${dmg_path}"

  submit_notarization "${dmg_path}"
  xcrun stapler staple "${dmg_path}"
  xcrun stapler validate "${dmg_path}"
  assess_gatekeeper_nonfatal "DMG" spctl --assess --type open --context context:primary-signature -vv "${dmg_path}"
  hdiutil verify "${dmg_path}" >/dev/null
}

if [[ ! -d "${APP_PATH}" ]]; then
  echo "Missing app bundle: ${APP_PATH}" >&2
  echo "Run pnpm delivery:macos first." >&2
  exit 1
fi

if [[ ! -f "${ZIP_PATH}" ]]; then
  echo "Missing delivery zip: ${ZIP_PATH}" >&2
  echo "Run pnpm delivery:macos first." >&2
  exit 1
fi

# Verify the exact current candidate before uploading anything. Recreate its
# archive so a stale zip cannot be notarized in place of the checked bundle.
node "${SOURCE_RECEIPT_TOOL}" --verify --repo-root "${REPO_ROOT}" --app "${APP_PATH}"
verify_macos_release_identity "${APP_PATH}"
SIGN_IDENTITY="$(detect_sign_identity || true)"
[[ -n "${SIGN_IDENTITY}" ]] || { echo 'Missing pinned-team signing identity' >&2; exit 1; }
ditto -c -k --sequesterRsrc --keepParent "${APP_PATH}" "${ZIP_PATH}"
shasum -a 256 "${ZIP_PATH}"
submit_notarization "${ZIP_PATH}"

xcrun stapler staple "${APP_PATH}"
xcrun stapler validate "${APP_PATH}"
assert_required_app_entitlements
assess_gatekeeper_nonfatal "App" spctl --assess --type execute -vv "${APP_PATH}"
ditto -c -k --sequesterRsrc --keepParent "${APP_PATH}" "${ZIP_PATH}"
rebuild_delivery_dmg
notarize_delivery_dmg

printf 'Notarized app bundle restapled and rezipped:\n'
printf ' - %s\n' "${APP_PATH}"
printf ' - %s\n' "${ZIP_PATH}"
printf ' - %s\n' "$(find_delivery_dmg)"
