#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

APP_NAME="${CUNZHI_MACOS_APP_NAME:-iterate}"
SOURCE_APP="${CUNZHI_MACOS_SOURCE_APP:-${REPO_ROOT}/target/release/bundle/macos/${APP_NAME}.app}"
DEST_APP="${CUNZHI_MACOS_DEST_APP:-/Applications/${APP_NAME}.app}"
SIGN_IDENTITY="${CUNZHI_MACOS_DEV_SIGN_IDENTITY:-}"
ENTITLEMENTS_PATH="${CUNZHI_MACOS_ENTITLEMENTS_PATH:-${REPO_ROOT}/Entitlements.plist}"
SIGN_TIMESTAMP="${CUNZHI_MACOS_CODESIGN_TIMESTAMP:-}"
SOURCE_RECEIPT_TOOL="${REPO_ROOT}/scripts/macos-source-receipt.mjs"
RECOVERY_TOOL="${REPO_ROOT}/scripts/macos-recovery.mjs"
BACKUP_DIR="${CUNZHI_MACOS_BACKUP_DIR:-${HOME}/.cunzhi/backups/desktop-install-$(date +%Y%m%d%H%M%S)-$$}"
INSTALL_CHANGED=0
canonical_pid=""
FN_OWNER_LOCK_SUFFIX="/speech/fn-owner.lock"

DO_BUILD=1
DO_OPEN=1
DO_SIGN=1
STOP_BACKGROUND=0
RESTART_BRIDGE=0
RESTART_RELAY=0

usage() {
  cat <<EOF
Install the local macOS desktop app bundle for development.

Usage:
  scripts/install-macos-dev-app.sh [options]

Options:
  --skip-build              Install the existing app bundle without running pnpm tauri:build
  --no-open                 Do not open the installed app after installation
  --no-sign                 Do not sign the installed app
  --stop-background         Also stop preserved serve/popup/bridge/relay processes
  --restart-bridge          Restart the preserved bridge LaunchAgent after install
  --restart-relay           Restart the preserved relay LaunchAgent after install
  --no-restart-background   Compatibility alias: clear explicit background restart requests
  --source-app <path>       Source app bundle path (default: target/release/bundle/macos/iterate.app)
  --dest-app <path>         Destination app bundle path (default: /Applications/iterate.app)
  -h, --help                Show this help

Environment:
  CUNZHI_MACOS_DEV_SIGN_IDENTITY   codesign identity for local installs (default: auto-detect)
  CUNZHI_MACOS_ALLOW_ADHOC_SIGN    set to 1 to allow fallback ad-hoc signing
  CUNZHI_MACOS_ENTITLEMENTS_PATH   app entitlements plist (default: Entitlements.plist)
  CUNZHI_MACOS_CODESIGN_TIMESTAMP  timestamp mode: default, none, or timestamp URL
  CUNZHI_MACOS_SOURCE_APP          source app bundle override
  CUNZHI_MACOS_DEST_APP            destination app bundle override
  CUNZHI_MACOS_ALLOW_DIRTY_SOURCE  set to 1 only for an intentional dirty development receipt
  CUNZHI_RELEASE_BRANCH            release mainline the source must contain (default: release/desktop)
  CUNZHI_MACOS_ALLOW_LINEAGE_REGRESSION  set to 1 to install even if the source lacks the mainline
                                   or the currently installed commit (P-2026-9347)
EOF
}

die() {
  echo "error: $*" >&2
  exit 1
}

info() {
  printf '==> %s\n' "$*"
}

restore_failed_install() {
  local status=$?
  trap - EXIT
  if [[ "${status}" -ne 0 && "${INSTALL_CHANGED}" -eq 1 ]]; then
    echo "Installation verification failed; restoring verified pre-install bundle" >&2
    if [[ -n "${canonical_pid}" ]] && kill -0 "${canonical_pid}" 2>/dev/null; then
      kill "${canonical_pid}" 2>/dev/null || true
    fi
    if node "${RECOVERY_TOOL}" restore --receipt "${BACKUP_DIR}/receipt.json" --app "${DEST_APP}"; then
      restart_requested_background_processes || true
      if [[ "${DO_OPEN}" -eq 1 ]]; then open -n "${DEST_APP}" || true; fi
    else
      echo "Automatic restore failed; verified backup retained at ${BACKUP_DIR}" >&2
    fi
  fi
  exit "${status}"
}
trap restore_failed_install EXIT

resolve_path() {
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

regex_escape() {
  sed 's/[][(){}.^$*+?|\\]/\\&/g' <<<"$1"
}

read_matching_pids() {
  local pattern="$1"

  pgrep -f "${pattern}" 2>/dev/null || true
}

process_command() {
  local pid="$1"

  ps -p "${pid}" -o command= 2>/dev/null || true
}

detect_sign_identity() {
  if [[ -n "${SIGN_IDENTITY}" ]]; then
    printf '%s\n' "${SIGN_IDENTITY}"
    return 0
  fi

  security find-identity -v -p codesigning \
    | sed -n 's/.*"\(Developer ID Application:.*\)"/\1/p' \
    | head -n 1
}

detect_development_sign_identity() {
  security find-identity -v -p codesigning \
    | sed -n 's/.*"\(Apple Development:.*\)"/\1/p' \
    | head -n 1
}

codesign_timestamp_args() {
  case "${SIGN_TIMESTAMP}" in
    "" | default)
      printf '%s\n' "--timestamp"
      ;;
    none)
      printf '%s\n' "--timestamp=none"
      ;;
    *)
      printf '%s\n' "--timestamp=${SIGN_TIMESTAMP}"
      ;;
  esac
}

wait_for_pids_exit() {
  local pid
  local still_running

  for _ in {1..30}; do
    still_running=0
    for pid in "$@"; do
      if kill -0 "${pid}" 2>/dev/null; then
        still_running=1
      fi
    done

    if [[ "${still_running}" -eq 0 ]]; then
      return 0
    fi
    sleep 0.2
  done

  return 1
}

stop_installed_app() {
  local dest_bin="${DEST_APP}/Contents/MacOS/${APP_NAME}"
  local pattern
  local pids
  local pid
  local command
  local stop_pids=()
  local preserved_pids=()

  pattern="$(regex_escape "${dest_bin}")"
  pids="$(read_matching_pids "${pattern}")"
  if [[ -z "${pids}" ]]; then
    info "No running installed app process found"
    return 0
  fi

  for pid in ${pids}; do
    command="$(process_command "${pid}")"
    if [[ -z "${command}" ]]; then
      continue
    fi

    if should_stop_app_process "${pid}"; then
      stop_pids+=("${pid}")
    else
      preserved_pids+=("${pid}")
    fi
  done

  if [[ "${#preserved_pids[@]}" -gt 0 ]]; then
    info "Preserving installed non-owner app processes: ${preserved_pids[*]}"
  fi

  if [[ "${#stop_pids[@]}" -eq 0 ]]; then
    info "No foreground installed app processes to stop"
    return 0
  fi

  info "Stopping installed foreground app processes: ${stop_pids[*]}"
  printf '%s\n' "${stop_pids[@]}" | xargs kill 2>/dev/null || true

  if wait_for_pids_exit "${stop_pids[@]}"; then
    return 0
  fi

  local remaining_stop_pids=()
  for pid in "${stop_pids[@]}"; do
    if kill -0 "${pid}" 2>/dev/null; then
      remaining_stop_pids+=("${pid}")
    fi
  done

  if [[ "${#remaining_stop_pids[@]}" -gt 0 ]]; then
    info "Force stopping remaining installed app processes"
    printf '%s\n' "${remaining_stop_pids[@]}" | xargs kill -9 2>/dev/null || true
  fi
}

stop_conflicting_foreground_app_bundles() {
  local pattern
  local pids
  local pid
  local command
  local stop_pids=()
  local preserved_pids=()

  pattern="/Contents/MacOS/$(regex_escape "${APP_NAME}")"
  pids="$(read_matching_pids "${pattern}")"
  if [[ -z "${pids}" ]]; then
    return 0
  fi

  for pid in ${pids}; do
    command="$(process_command "${pid}")"
    if [[ -z "${command}" ]]; then
      continue
    fi

    if should_stop_app_process "${pid}"; then
      stop_pids+=("${pid}")
    else
      preserved_pids+=("${pid}")
    fi
  done

  if [[ "${#preserved_pids[@]}" -gt 0 ]]; then
    info "Preserving non-owner app bundle processes: ${preserved_pids[*]}"
  fi

  if [[ "${#stop_pids[@]}" -eq 0 ]]; then
    return 0
  fi

  info "Stopping conflicting foreground app bundle processes: ${stop_pids[*]}"
  printf '%s\n' "${stop_pids[@]}" | xargs kill 2>/dev/null || true

  if wait_for_pids_exit "${stop_pids[@]}"; then
    return 0
  fi

  local remaining_stop_pids=()
  for pid in "${stop_pids[@]}"; do
    if kill -0 "${pid}" 2>/dev/null; then
      remaining_stop_pids+=("${pid}")
    fi
  done

  if [[ "${#remaining_stop_pids[@]}" -gt 0 ]]; then
    info "Force stopping remaining conflicting app bundle processes"
    printf '%s\n' "${remaining_stop_pids[@]}" | xargs kill -9 2>/dev/null || true
  fi
}

bundle_has_running_code() {
  local bundle="$1"
  local bundle_path
  local hosting_path
  local pid
  local pids

  bundle_path="$(cd "$(dirname "${bundle}")" && pwd -P)/$(basename "${bundle}")"
  pids="$({ pgrep -x "${APP_NAME}" 2>/dev/null || true; pgrep -x "mcp-server" 2>/dev/null || true; } | sort -u)"
  if [[ -z "${pids}" ]]; then
    return 1
  fi

  for pid in ${pids}; do
    while IFS= read -r hosting_path; do
      if [[ "${hosting_path}" == "${bundle_path}/Contents/MacOS/"* ]]; then
        return 0
      fi
    done < <(codesign -h "${pid}" 2>/dev/null || true)
  done

  return 1
}

cleanup_retired_apps() {
  local retired_root="$1"
  local retired_app

  [[ -d "${retired_root}" ]] || return 0
  for retired_app in "${retired_root}"/*.app; do
    [[ -d "${retired_app}" ]] || continue
    if bundle_has_running_code "${retired_app}"; then
      info "Keeping retired app bundle used by running signed code: ${retired_app}"
    else
      info "Removing unused retired app bundle: ${retired_app}"
      rm -rf "${retired_app}"
    fi
  done
  rmdir "${retired_root}" 2>/dev/null || true
}

copy_app() {
  local dest_parent
  local retired_app=""
  local retired_root
  local staging_app

  [[ -d "${SOURCE_APP}" ]] || die "missing source app bundle: ${SOURCE_APP}"
  [[ -f "${SOURCE_APP}/Contents/MacOS/${APP_NAME}" ]] || die "missing source app binary: ${SOURCE_APP}/Contents/MacOS/${APP_NAME}"

  dest_parent="$(dirname "${DEST_APP}")"
  retired_root="${dest_parent}/.${APP_NAME}-retired"
  staging_app="${dest_parent}/.${APP_NAME}-installing-$$.app"

  mkdir -p "${dest_parent}"
  cleanup_retired_apps "${retired_root}"
  [[ ! -e "${staging_app}" ]] || die "staging app path already exists: ${staging_app}"

  info "Staging ${SOURCE_APP} -> ${staging_app}"
  if ! ditto "${SOURCE_APP}" "${staging_app}"; then
    rm -rf "${staging_app}"
    die "failed to stage app bundle"
  fi

  if [[ -e "${DEST_APP}" ]]; then
    mkdir -p "${retired_root}"
    retired_app="${retired_root}/${APP_NAME}-$(date +%Y%m%d%H%M%S)-$$.app"
    info "Retiring installed bundle before replacement: ${DEST_APP} -> ${retired_app}"
    mv "${DEST_APP}" "${retired_app}"
  fi

  info "Installing staged app -> ${DEST_APP}"
  if ! mv "${staging_app}" "${DEST_APP}"; then
    if [[ -n "${retired_app}" && ! -e "${DEST_APP}" ]]; then
      mv "${retired_app}" "${DEST_APP}" || true
    fi
    rm -rf "${staging_app}"
    die "failed to install staged app bundle"
  fi
  INSTALL_CHANGED=1

  if [[ -n "${retired_app}" ]]; then
    if bundle_has_running_code "${retired_app}"; then
      info "Preserving retired bundle for running signed processes: ${retired_app}"
    else
      info "Removing retired bundle with no running code: ${retired_app}"
      rm -rf "${retired_app}"
      rmdir "${retired_root}" 2>/dev/null || true
    fi
  fi
}

clear_copied_app_attributes() {
  info "Clearing copied app extended attributes before signing"
  xattr -cr "${DEST_APP}"
}

write_or_verify_source_receipt() {
  [[ -f "${SOURCE_RECEIPT_TOOL}" ]] || die "missing source receipt tool: ${SOURCE_RECEIPT_TOOL}"

  if [[ "${DO_BUILD}" -eq 1 ]]; then
    info "Writing source receipt into freshly built app"
    node "${SOURCE_RECEIPT_TOOL}" --repo-root "${REPO_ROOT}" --app "${SOURCE_APP}"
  else
    info "Verifying existing source receipt before skip-build install"
    node "${SOURCE_RECEIPT_TOOL}" --verify --repo-root "${REPO_ROOT}" --app "${SOURCE_APP}"
  fi
}

sign_app() {
  if [[ "${DO_SIGN}" -ne 1 ]]; then
    return 0
  fi

  local identity
  local binary_path
  local timestamp_args=()

  identity="$(detect_sign_identity || true)"
  if [[ -z "${identity}" ]]; then
    identity="$(detect_development_sign_identity || true)"
  fi

  if [[ -z "${identity}" ]]; then
    if [[ "${CUNZHI_MACOS_ALLOW_ADHOC_SIGN:-0}" == "1" ]]; then
      identity="-"
      timestamp_args=(--timestamp=none)
      info "No codesign identity found; using explicit ad-hoc signing because CUNZHI_MACOS_ALLOW_ADHOC_SIGN=1"
    else
      die "no codesign identity found; set CUNZHI_MACOS_DEV_SIGN_IDENTITY, install a Developer ID/Apple Development certificate, or pass --no-sign"
    fi
  else
    if [[ "${identity}" == "-" ]]; then
      [[ "${CUNZHI_MACOS_ALLOW_ADHOC_SIGN:-0}" == "1" ]] || die "ad-hoc signing is disabled by default; set CUNZHI_MACOS_ALLOW_ADHOC_SIGN=1 to use identity '-'"
      timestamp_args=(--timestamp=none)
      info "Using explicit ad-hoc signing because CUNZHI_MACOS_ALLOW_ADHOC_SIGN=1"
    else
      while IFS= read -r timestamp_arg; do
        timestamp_args+=("${timestamp_arg}")
      done < <(codesign_timestamp_args)
    fi
  fi

  [[ -f "${ENTITLEMENTS_PATH}" ]] || die "missing macOS entitlements file: ${ENTITLEMENTS_PATH}"

  info "Signing installed app binaries with identity: ${identity}"
  for binary_path in "${DEST_APP}/Contents/MacOS"/* \
    "${DEST_APP}/Contents/Resources/managed-frpc" \
    "${DEST_APP}/Contents/Resources/managed-proxy"; do
    [[ -f "${binary_path}" ]] || continue
    if [[ "$(basename "${binary_path}")" == "mcp-server" ]]; then
      codesign --force --options runtime "${timestamp_args[@]}" \
        --identifier "com.kexin94yyds.iterate.mcp-server" \
        --sign "${identity}" "${binary_path}"
    else
      codesign --force --options runtime "${timestamp_args[@]}" --sign "${identity}" "${binary_path}"
    fi
  done

  info "Signing installed app bundle with entitlements: ${ENTITLEMENTS_PATH}"
  codesign \
    --force \
    --options runtime \
    "${timestamp_args[@]}" \
    --entitlements "${ENTITLEMENTS_PATH}" \
    --sign "${identity}" \
    "${DEST_APP}"
}

verify_app() {
  local dest_bin="${DEST_APP}/Contents/MacOS/${APP_NAME}"

  info "Clearing quarantine attributes"
  xattr -cr "${DEST_APP}"

  info "Verifying code signature"
  codesign --verify --deep --strict "${DEST_APP}"

  info "Checking installed app binary can execute"
  "${dest_bin}" --version >/dev/null

  info "Checking installed frontend assets"
  node "${RECOVERY_TOOL}" production --app "${DEST_APP}"
  "${dest_bin}" --check-frontend-assets >/dev/null

  info "Verifying installed source receipt"
  node "${SOURCE_RECEIPT_TOOL}" --verify --repo-root "${REPO_ROOT}" --app "${DEST_APP}"
}

restart_requested_background_processes() {
  if [[ "${STOP_BACKGROUND}" -eq 1 ]]; then
    return 0
  fi

  local labels=()
  if [[ "${RESTART_BRIDGE}" -eq 1 ]]; then
    labels+=("com.cunzhi.iterate.bridge")
  fi
  if [[ "${RESTART_RELAY}" -eq 1 ]]; then
    labels+=("com.cunzhi.iterate.relay-mac-client")
  fi
  if [[ "${#labels[@]}" -eq 0 ]]; then
    return 0
  fi

  local uid
  local label
  local service
  local restarted=0

  uid="$(id -u)"
  for label in "${labels[@]}"; do
    service="gui/${uid}/${label}"

    if launchctl print "${service}" >/dev/null 2>&1; then
      info "Restarting preserved background LaunchAgent: ${label}"
      launchctl kickstart -k "${service}"
      restarted=1
    else
      info "Preserved background LaunchAgent not loaded, leaving process owner unchanged: ${label}"
    fi
  done

  if [[ "${restarted}" -eq 1 ]]; then
    sleep 1
  fi
}

canonical_gui_pids() {
  local dest_bin="${DEST_APP}/Contents/MacOS/${APP_NAME}"
  local pattern
  local pid
  local command

  pattern="$(regex_escape "${dest_bin}")"
  for pid in $(read_matching_pids "${pattern}"); do
    command="$(process_command "${pid}")"
    if [[ "${command}" == "${dest_bin}" ]]; then
      printf '%s\n' "${pid}"
    fi
  done
}

fn_owner_lock_for_pid() {
  local pid="$1"

  lsof -a -p "${pid}" -Fn 2>/dev/null \
    | sed -n 's/^n//p' \
    | grep "$(regex_escape "${FN_OWNER_LOCK_SUFFIX}")$" \
    | head -n 1
}

fn_owner_metadata_matches() {
  local lock_path="$1"
  local pid="$2"
  local expected_executable="${3:-}"

  node -e '
    const fs = require("node:fs")
    const [lockPath, expectedPid, expectedExecutable] = process.argv.slice(1)
    try {
      const owner = JSON.parse(fs.readFileSync(lockPath, "utf8"))
      if (
        String(owner.pid) !== expectedPid
        || owner.role !== "canonical-gui"
        || (expectedExecutable && owner.executable !== expectedExecutable)
      ) process.exit(1)
    } catch {
      process.exit(1)
    }
  ' "${lock_path}" "${pid}" "${expected_executable}"
}

is_canonical_gui_process() {
  local pid="$1"
  local lock_path

  lock_path="$(fn_owner_lock_for_pid "${pid}")"
  [[ -n "${lock_path}" ]] && fn_owner_metadata_matches "${lock_path}" "${pid}"
}

should_stop_app_process() {
  local pid="$1"

  # Standalone popups may have no distinguishing command-line flag. Default to
  # preserving every non-owner process and stop all roles only when requested.
  [[ "${STOP_BACKGROUND}" -eq 1 ]] || is_canonical_gui_process "${pid}"
}

wait_for_canonical_gui_owner() {
  local dest_bin="${DEST_APP}/Contents/MacOS/${APP_NAME}"
  local attempt
  local pid
  local lock_path

  for attempt in {1..40}; do
    for pid in $(canonical_gui_pids); do
      lock_path="$(fn_owner_lock_for_pid "${pid}")"
      if [[ -n "${lock_path}" ]] && fn_owner_metadata_matches "${lock_path}" "${pid}" "${dest_bin}"; then
        printf '%s|%s\n' "${pid}" "${lock_path}"
        return 0
      fi
    done
    sleep 0.25
  done

  return 1
}

print_status() {
  local dest_bin="${DEST_APP}/Contents/MacOS/${APP_NAME}"
  local pattern
  local pids

  info "Installed bundle timestamps"
  stat -f 'source: %Sm %N' "${SOURCE_APP}"
  stat -f 'dest:   %Sm %N' "${DEST_APP}"

  pattern="$(regex_escape "${dest_bin}")"
  pids="$(read_matching_pids "${pattern}")"
  if [[ -n "${pids}" ]]; then
    info "Running installed app processes"
    pgrep -fl "${pattern}" || true
  fi
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --)
      shift
      ;;
    --skip-build)
      DO_BUILD=0
      shift
      ;;
    --no-open)
      DO_OPEN=0
      shift
      ;;
    --no-sign)
      DO_SIGN=0
      shift
      ;;
    --stop-background)
      STOP_BACKGROUND=1
      shift
      ;;
    --restart-bridge)
      RESTART_BRIDGE=1
      shift
      ;;
    --restart-relay)
      RESTART_RELAY=1
      shift
      ;;
    --no-restart-background)
      RESTART_BRIDGE=0
      RESTART_RELAY=0
      shift
      ;;
    --source-app)
      [[ "$#" -ge 2 ]] || die "--source-app requires a path"
      SOURCE_APP="$(resolve_path "$2")"
      shift 2
      ;;
    --dest-app)
      [[ "$#" -ge 2 ]] || die "--dest-app requires a path"
      DEST_APP="$(resolve_path "$2")"
      shift 2
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
done

if [[ "${STOP_BACKGROUND}" -eq 1 ]] && { [[ "${RESTART_BRIDGE}" -eq 1 ]] || [[ "${RESTART_RELAY}" -eq 1 ]]; }; then
  die "--stop-background cannot be combined with --restart-bridge or --restart-relay"
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
  die "macOS is required"
fi

if [[ "${DO_BUILD}" -eq 1 ]]; then
  # shellcheck source=scripts/managed-build-env.sh
  source "${REPO_ROOT}/scripts/managed-build-env.sh"
  load_managed_build_env ${CUNZHI_REQUIRE_MANAGED:+--require} || die "managed service configuration invalid"
  info "Building desktop app"
  (cd "${REPO_ROOT}" && pnpm tauri:build)
else
  info "Skipping build"
fi

write_or_verify_source_receipt
info "Checking release lineage (mainline + installed commit)"
bash "${REPO_ROOT}/scripts/check-release-lineage.sh" "${REPO_ROOT}" "${SOURCE_APP}" "${DEST_APP}"
info "Verifying production mode before touching the installed app"
node "${RECOVERY_TOOL}" production --app "${SOURCE_APP}"
if [[ -d "${DEST_APP}" ]]; then
  info "Creating and verifying the recovery bundle"
  node "${RECOVERY_TOOL}" backup --app "${DEST_APP}" --out "${BACKUP_DIR}"
fi
stop_installed_app
stop_conflicting_foreground_app_bundles
copy_app
clear_copied_app_attributes
sign_app
verify_app
restart_requested_background_processes
info "Retiring idle serve processes still running the previous binary"
bash "${REPO_ROOT}/scripts/retire-stale-serves.sh" "${DEST_APP}" || true

if [[ "${DO_OPEN}" -eq 1 ]]; then
  launch_since_ms="$(node -p 'Date.now()')"
  info "Opening installed app as a new canonical GUI instance"
  open -n "${DEST_APP}"
  if canonical_owner="$(wait_for_canonical_gui_owner)"; then
    canonical_pid="${canonical_owner%%|*}"
    canonical_lock="${canonical_owner#*|}"
    info "canonical-gui Fn owner ready: pid=${canonical_pid} lock=${canonical_lock}"
  else
    print_status
    die "installed app did not establish a canonical-gui Fn owner"
  fi
  node "${RECOVERY_TOOL}" wait-ready --app "${DEST_APP}" --pid "${canonical_pid}" --since "${launch_since_ms}"
fi

print_status
if [[ "${DO_OPEN}" -eq 1 ]]; then
  info "Installed and new production UI ready; human behavior acceptance remains separate"
else
  info "Installed; GUI readiness verification pending"
fi
