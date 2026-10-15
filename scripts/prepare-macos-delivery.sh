#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
source "${SCRIPT_DIR}/macos-release-identity.sh"
export CUNZHI_MACOS_ALLOW_DIRTY_SOURCE=0

resolve_repo_path() {
  local path="$1"

  case "${path}" in
    /*)
      printf '%s\n' "${path}"
      ;;
    *)
      printf '%s/%s\n' "${REPO_ROOT}" "${path}"
      ;;
  esac
}

resolve_cargo_target_dir() {
  local target_dir="${CARGO_TARGET_DIR:-}"
  local metadata_target_dir

  if [[ -n "${target_dir}" ]]; then
    resolve_repo_path "${target_dir}"
    return 0
  fi

  metadata_target_dir="$(
    cargo metadata --manifest-path "${REPO_ROOT}/Cargo.toml" --format-version 1 --no-deps 2>/dev/null \
      | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p' \
      | head -n 1
  )"

  if [[ -n "${metadata_target_dir}" ]]; then
    printf '%s\n' "${metadata_target_dir}"
  else
    printf '%s/target\n' "${REPO_ROOT}"
  fi
}

CARGO_TARGET_DIR_RESOLVED="$(resolve_cargo_target_dir)"
PROMPT_SOURCE="${REPO_ROOT}/docs/release/INSTALL_PROMPT.md"
INSTALLATION_SOURCE="${REPO_ROOT}/docs/release/INSTALLATION.md"
BUNDLE_MACOS_DIR="${CARGO_TARGET_DIR_RESOLVED}/release/bundle/macos"
BUNDLE_DMG_DIR="${CARGO_TARGET_DIR_RESOLVED}/release/bundle/dmg"
DELIVERY_DIR="$(resolve_repo_path "${CUNZHI_MACOS_DELIVERY_DIR:-${CARGO_TARGET_DIR_RESOLVED}/release/delivery/macos}")"
APP_PATH="${BUNDLE_MACOS_DIR}/iterate.app"
RELEASE_ITERATE_BIN="${CARGO_TARGET_DIR_RESOLVED}/release/iterate"
ENTITLEMENTS_PATH="${REPO_ROOT}/Entitlements.plist"
SOURCE_RECEIPT_TOOL="${REPO_ROOT}/scripts/macos-source-receipt.mjs"
SIGN_IDENTITY="${CUNZHI_MACOS_SIGN_IDENTITY:-}"
SIGN_TIMESTAMP="${CUNZHI_MACOS_CODESIGN_TIMESTAMP:-}"
ALLOWED_RELEASE_BIN_NAMES=("iterate" "mcp-server")

detect_sign_identity() {
  resolve_macos_release_sign_identity "${SIGN_IDENTITY}"
}

is_allowed_release_bin() {
  local bin_name="$1"
  local allowed_bin_name

  for allowed_bin_name in "${ALLOWED_RELEASE_BIN_NAMES[@]}"; do
    if [[ "${bin_name}" == "${allowed_bin_name}" ]]; then
      return 0
    fi
  done

  return 1
}

assert_release_bundle_contents() {
  local bundle_root="${APP_PATH}/Contents/MacOS"
  local binary_path
  local missing_or_unexpected=0

  for allowed_bin_name in "${ALLOWED_RELEASE_BIN_NAMES[@]}"; do
    if [[ ! -f "${bundle_root}/${allowed_bin_name}" ]]; then
      echo "Missing required release binary: ${bundle_root}/${allowed_bin_name}" >&2
      missing_or_unexpected=1
    fi
  done

  for binary_path in "${bundle_root}"/*; do
    [[ -e "${binary_path}" ]] || continue
    [[ -f "${binary_path}" ]] || continue

    if ! is_allowed_release_bin "$(basename "${binary_path}")"; then
      echo "Unexpected release binary in app bundle: ${binary_path}" >&2
      missing_or_unexpected=1
    fi
  done

  if [[ "${missing_or_unexpected}" -ne 0 ]]; then
    exit 1
  fi
}

sign_app_bundle() {
  local identity="$1"
  local binary_path
  local timestamp_args=()

  case "${SIGN_TIMESTAMP}" in
    "" | default)
      timestamp_args=(--timestamp)
      ;;
    none)
      timestamp_args=(--timestamp=none)
      ;;
    *)
      timestamp_args=(--timestamp="${SIGN_TIMESTAMP}")
      ;;
  esac

  if [[ ! -f "${ENTITLEMENTS_PATH}" ]]; then
    echo "Missing macOS entitlements file: ${ENTITLEMENTS_PATH}" >&2
    exit 1
  fi

  for binary_path in "${APP_PATH}/Contents/MacOS"/* \
    "${APP_PATH}/Contents/Resources/managed-frpc" \
    "${APP_PATH}/Contents/Resources/managed-proxy"; do
    [[ -f "${binary_path}" ]] || continue
    if [[ "$(basename "${binary_path}")" == "mcp-server" ]]; then
      codesign --force --options runtime "${timestamp_args[@]}" \
        --identifier "com.kexin94yyds.iterate.mcp-server" \
        --sign "${identity}" "${binary_path}"
    else
      codesign --force --options runtime "${timestamp_args[@]}" --sign "${identity}" "${binary_path}"
    fi
  done

  codesign \
    --force \
    --options runtime \
    "${timestamp_args[@]}" \
    --entitlements "${ENTITLEMENTS_PATH}" \
    --sign "${identity}" \
    "${APP_PATH}"
  codesign --verify --deep --strict --verbose=2 "${APP_PATH}"
  verify_macos_release_identity "${APP_PATH}"
  assert_required_app_entitlements

  if spctl --assess --type execute -vv "${APP_PATH}"; then
    printf 'Gatekeeper assessment: accepted\n'
  else
    printf 'Gatekeeper assessment: not yet accepted (likely needs notarization)\n'
  fi
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

  # 不同 macOS 版本的 plutil -p 把布尔值打印成 true 或 1（CI 的 macos-14 是 1）。
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

create_delivery_dmg() {
  local source_dmg_path="$1"
  local output_dmg_path="${DELIVERY_DIR}/$(basename "${source_dmg_path}")"
  local stage_dir

  stage_dir="$(mktemp -d "${TMPDIR:-/tmp}/iterate-dmg-stage.XXXXXX")"
  trap 'rm -rf "${stage_dir}"' RETURN

  cp -R "${APP_PATH}" "${stage_dir}/iterate.app"

  if [[ -f "${INSTALLATION_SOURCE}" ]]; then
    cp "${INSTALLATION_SOURCE}" "${stage_dir}/INSTALLATION.md"
  fi

  if [[ -f "${PROMPT_SOURCE}" ]]; then
    cp "${PROMPT_SOURCE}" "${stage_dir}/INSTALL_PROMPT.md"
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

prune_release_bundle() {
  local bundle_root="${APP_PATH}/Contents/MacOS"
  local binary_path
  local bin_name

  [[ -d "${bundle_root}" ]] || return 0

  for binary_path in "${bundle_root}"/*; do
    [[ -e "${binary_path}" ]] || continue
    [[ -f "${binary_path}" ]] || continue

    bin_name="$(basename "${binary_path}")"
    if ! is_allowed_release_bin "${bin_name}"; then
      printf 'Pruning extra release binary: %s\n' "${binary_path}"
      rm -f "${binary_path}"
    fi
  done
}

cd "${REPO_ROOT}"
# 官方签名交付必须带托管服务配置；在清理产物目录之前检查。
# shellcheck source=scripts/managed-build-env.sh
source "${REPO_ROOT}/scripts/managed-build-env.sh"
load_managed_build_env --require || exit 1
SIGN_IDENTITY="$(detect_sign_identity || true)"
if [[ -z "${SIGN_IDENTITY}" ]]; then
  echo "No Developer ID identity for Team UM3Z9G5DNH; refusing release packaging." >&2
  exit 1
fi
mkdir -p "${BUNDLE_DMG_DIR}" "${DELIVERY_DIR}"
find "${BUNDLE_DMG_DIR}" -maxdepth 1 -type f -name '*.dmg' -delete
find "${DELIVERY_DIR}" -maxdepth 1 -type f \( -name '*.dmg' -o -name '*.zip' \) -delete
pnpm tauri:build

if [[ ! -f "${PROMPT_SOURCE}" ]]; then
  echo "Missing install prompt source: ${PROMPT_SOURCE}" >&2
  exit 1
fi

if [[ ! -f "${INSTALLATION_SOURCE}" ]]; then
  echo "Missing installation prompt source: ${INSTALLATION_SOURCE}" >&2
  exit 1
fi

if [[ ! -d "${APP_PATH}" ]]; then
  echo "Missing built app: ${APP_PATH}" >&2
  exit 1
fi

if [[ ! -x "${RELEASE_ITERATE_BIN}" ]]; then
  echo "Missing built iterate binary: ${RELEASE_ITERATE_BIN}" >&2
  exit 1
fi

ACTIVATION_GATE_STATUS="$("${RELEASE_ITERATE_BIN}" --activation-gate-status)"
if [[ "${ACTIVATION_GATE_STATUS}" != "activation_gate_required=false" ]]; then
  echo "Community macOS artifact unexpectedly requires activation: ${ACTIVATION_GATE_STATUS}" >&2
  exit 1
fi

"${RELEASE_ITERATE_BIN}" --check-frontend-assets --frontend-dist "${REPO_ROOT}/dist"

DMG_PATH="$(find "${BUNDLE_DMG_DIR}" -maxdepth 1 -type f -name '*.dmg' -print -quit)"

if [[ -z "${DMG_PATH}" ]]; then
  echo "No DMG found in ${BUNDLE_DMG_DIR}" >&2
  exit 1
fi

prune_release_bundle
assert_release_bundle_contents

printf 'Writing clean source receipt before final signing.\n'
node "${SOURCE_RECEIPT_TOOL}" --repo-root "${REPO_ROOT}" --app "${APP_PATH}"

printf 'Signing app with Developer ID identity: %s\n' "${SIGN_IDENTITY}"
sign_app_bundle "${SIGN_IDENTITY}"

printf 'Verifying source receipt after final signing.\n'
node "${SOURCE_RECEIPT_TOOL}" --verify --repo-root "${REPO_ROOT}" --app "${APP_PATH}"

mkdir -p "${DELIVERY_DIR}"

cp "${PROMPT_SOURCE}" "${DELIVERY_DIR}/INSTALL_PROMPT.md"
cp "${PROMPT_SOURCE}" "${BUNDLE_MACOS_DIR}/INSTALL_PROMPT.md"
cp "${PROMPT_SOURCE}" "${BUNDLE_DMG_DIR}/INSTALL_PROMPT.md"
cp "${INSTALLATION_SOURCE}" "${DELIVERY_DIR}/INSTALLATION.md"
cp "${INSTALLATION_SOURCE}" "${BUNDLE_MACOS_DIR}/INSTALLATION.md"
cp "${INSTALLATION_SOURCE}" "${BUNDLE_DMG_DIR}/INSTALLATION.md"
rm -f "${DELIVERY_DIR}/iterate.app.zip"
ditto -c -k --sequesterRsrc --keepParent "${APP_PATH}" "${DELIVERY_DIR}/iterate.app.zip"
create_delivery_dmg "${DMG_PATH}"

printf 'Prepared macOS delivery files:\n'
printf ' - %s\n' "${DELIVERY_DIR}/iterate.app.zip"
printf ' - %s\n' "${DELIVERY_DIR}/INSTALLATION.md"
printf ' - %s\n' "${DELIVERY_DIR}/INSTALL_PROMPT.md"
printf ' - %s\n' "${DELIVERY_DIR}/$(basename "${DMG_PATH}")"
printf '\n'
printf 'Note: the app zip and DMG are prepared from the current app bundle after cleanup/signing.\n'
printf 'Run pnpm notarize:macos to staple the app and regenerate the final delivery DMG.\n'
