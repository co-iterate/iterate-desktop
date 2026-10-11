#!/usr/bin/env bash
set -euo pipefail

LABEL="${LABEL:-com.cunzhi.iterate.bridge}"
PORT="${PORT:-8080}"
WORKSPACE_PATH="${WORKSPACE_PATH:-${HOME}/cunzhi}"
if [ -z "${APP_BIN:-}" ]; then
  if [ -x "/Applications/iterate.app/Contents/MacOS/iterate" ]; then
    APP_BIN="/Applications/iterate.app/Contents/MacOS/iterate"
  elif [ -x "${WORKSPACE_PATH}/target/release/iterate" ]; then
    APP_BIN="${WORKSPACE_PATH}/target/release/iterate"
  elif [ -x "${HOME}/bin/iterate" ]; then
    APP_BIN="${HOME}/bin/iterate"
  elif [ -x "${HOME}/.local/bin/iterate" ]; then
    APP_BIN="${HOME}/.local/bin/iterate"
  else
    APP_BIN="/Applications/iterate.app/Contents/MacOS/iterate"
  fi
fi
PLIST_PATH="${PLIST_PATH:-${HOME}/Library/LaunchAgents/${LABEL}.plist}"
LOG_DIR="${LOG_DIR:-${HOME}/Library/Logs/iterate}"
OUT_LOG="${OUT_LOG:-${LOG_DIR}/bridge-daemon.out.log}"
ERR_LOG="${ERR_LOG:-${LOG_DIR}/bridge-daemon.err.log}"
ITERATE_TAILSCALE_IP="${ITERATE_TAILSCALE_IP:-}"
ITERATE_REQUIRE_MOBILE_AUTH="${ITERATE_REQUIRE_MOBILE_AUTH:-}"
ITERATE_PUBLIC_BRIDGE_BASE_URL="${ITERATE_PUBLIC_BRIDGE_BASE_URL:-}"
APNS_ENV_FILE="${APNS_ENV_FILE:-${HOME}/.config/iterate/apns-env.sh}"
BRIDGE_CODE_REQUIREMENT='identifier "com.kexin94yyds.iterate" and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = "UM3Z9G5DNH"'
DOMAIN="gui/$(id -u)"
SERVICE="${DOMAIN}/${LABEL}"

load_apns_env_defaults() {
  [ -f "${APNS_ENV_FILE}" ] || return 0

  local old_key_id="${APNS_KEY_ID:-}" old_team_id="${APNS_TEAM_ID:-}" old_key_path="${APNS_AUTH_KEY_PATH:-}"
  local old_topic="${APNS_TOPIC:-}" old_env="${APNS_ENV:-}"
  local old_sandbox_key_id="${APNS_SANDBOX_KEY_ID:-}" old_sandbox_key_path="${APNS_SANDBOX_AUTH_KEY_PATH:-}"
  local old_production_key_id="${APNS_PRODUCTION_KEY_ID:-}" old_production_key_path="${APNS_PRODUCTION_AUTH_KEY_PATH:-}"

  # apns-env.sh is documented as a sourceable private shell snippet.
  # Source it here, but preserve any explicit non-empty environment overrides.
  # shellcheck disable=SC1090
  source "${APNS_ENV_FILE}"

  if [ -n "${old_key_id}" ]; then APNS_KEY_ID="${old_key_id}"; fi
  if [ -n "${old_team_id}" ]; then APNS_TEAM_ID="${old_team_id}"; fi
  if [ -n "${old_key_path}" ]; then APNS_AUTH_KEY_PATH="${old_key_path}"; fi
  if [ -n "${old_topic}" ]; then APNS_TOPIC="${old_topic}"; fi
  if [ -n "${old_env}" ]; then APNS_ENV="${old_env}"; fi
  if [ -n "${old_sandbox_key_id}" ]; then APNS_SANDBOX_KEY_ID="${old_sandbox_key_id}"; fi
  if [ -n "${old_sandbox_key_path}" ]; then APNS_SANDBOX_AUTH_KEY_PATH="${old_sandbox_key_path}"; fi
  if [ -n "${old_production_key_id}" ]; then APNS_PRODUCTION_KEY_ID="${old_production_key_id}"; fi
  if [ -n "${old_production_key_path}" ]; then APNS_PRODUCTION_AUTH_KEY_PATH="${old_production_key_path}"; fi
}

load_apns_env_defaults

usage() {
  cat <<EOF
Usage: $(basename "$0") <command>

Commands:
  status      Show launchd, port, and local health state (default)
  doctor      Run non-destructive preflight checks
  render      Print the LaunchAgent plist to stdout
  install     Write the LaunchAgent plist, but do not load it
  load        Bootstrap and kickstart the LaunchAgent
  unload      Boot out the LaunchAgent
  restart     unload + load
  uninstall   unload and remove the LaunchAgent plist
  health      Probe local HTTP and WebSocket on PORT

Environment:
  LABEL=${LABEL}
  PORT=${PORT}
  APP_BIN=${APP_BIN}
  PLIST_PATH=${PLIST_PATH}
  LOG_DIR=${LOG_DIR}
  WORKSPACE_PATH=${WORKSPACE_PATH}
  ITERATE_TAILSCALE_IP  Override Tailscale IPv4 for daemon pairing (optional)
  ITERATE_REQUIRE_MOBILE_AUTH  Require paired mobile auth for public bridge access (set to 1 to enforce)
  ITERATE_PUBLIC_BRIDGE_BASE_URL  Override public bridge base URL for pairing/probes (optional)
  APNS_ENV_FILE=${APNS_ENV_FILE}
  ALLOW_PORT_IN_USE=1   Allow load/restart even if PORT is already listening
EOF
}

log() {
  printf '%s [bridge-daemon-install] %s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*"
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

require_app_bin() {
  [ -x "${APP_BIN}" ] || die "APP_BIN is not executable: ${APP_BIN}"
}

trusted_bridge_identity() {
  command -v codesign >/dev/null 2>&1 || return 1
  [ -x "${APP_BIN}" ] || return 1
  codesign --verify --strict --verbose=2 \
    "-R=${BRIDGE_CODE_REQUIREMENT}" \
    "${APP_BIN}" >/dev/null 2>&1
}

require_trusted_bridge_identity() {
  require_cmd codesign
  require_app_bin
  trusted_bridge_identity || die "APP_BIN does not satisfy the iterate Developer ID requirement; refusing to install or activate a Bridge that the auth broker will reject: ${APP_BIN}"
}

render_tailscale_ip_env() {
  if [ -n "${ITERATE_TAILSCALE_IP}" ]; then
    printf '      <key>ITERATE_TAILSCALE_IP</key>\n'
    printf '      <string>%s</string>\n' "${ITERATE_TAILSCALE_IP}"
  fi
}

render_mobile_auth_env() {
  if [ -n "${ITERATE_REQUIRE_MOBILE_AUTH}" ]; then
    printf '      <key>ITERATE_REQUIRE_MOBILE_AUTH</key>\n'
    printf '      <string>%s</string>\n' "${ITERATE_REQUIRE_MOBILE_AUTH}"
  fi
}

render_public_bridge_base_url_env() {
  if [ -n "${ITERATE_PUBLIC_BRIDGE_BASE_URL}" ]; then
    printf '      <key>ITERATE_PUBLIC_BRIDGE_BASE_URL</key>\n'
    printf '      <string>%s</string>\n' "${ITERATE_PUBLIC_BRIDGE_BASE_URL}"
  fi
}

render_env_var() {
  local key="$1"
  local value="$2"
  if [ -n "${value}" ]; then
    printf '      <key>%s</key>\n' "${key}"
    printf '      <string>%s</string>\n' "${value}"
  fi
}

render_apns_env_vars() {
  render_env_var "APNS_KEY_ID" "${APNS_KEY_ID:-}"
  render_env_var "APNS_TEAM_ID" "${APNS_TEAM_ID:-}"
  render_env_var "APNS_AUTH_KEY_PATH" "${APNS_AUTH_KEY_PATH:-}"
  render_env_var "APNS_SANDBOX_KEY_ID" "${APNS_SANDBOX_KEY_ID:-}"
  render_env_var "APNS_SANDBOX_AUTH_KEY_PATH" "${APNS_SANDBOX_AUTH_KEY_PATH:-}"
  render_env_var "APNS_PRODUCTION_KEY_ID" "${APNS_PRODUCTION_KEY_ID:-}"
  render_env_var "APNS_PRODUCTION_AUTH_KEY_PATH" "${APNS_PRODUCTION_AUTH_KEY_PATH:-}"
  render_env_var "APNS_TOPIC" "${APNS_TOPIC:-}"
  render_env_var "APNS_ENV" "${APNS_ENV:-}"
}

render_plist() {
  cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>${LABEL}</string>
    <key>ProgramArguments</key>
    <array>
      <string>${APP_BIN}</string>
      <string>--bridge-only</string>
      <string>--port</string>
      <string>${PORT}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ThrottleInterval</key>
    <integer>5</integer>
    <key>WorkingDirectory</key>
    <string>${WORKSPACE_PATH}</string>
    <key>EnvironmentVariables</key>
    <dict>
      <key>HOME</key>
      <string>${HOME}</string>
EOF
  render_tailscale_ip_env
  render_mobile_auth_env
  render_public_bridge_base_url_env
  render_apns_env_vars
  cat <<EOF
    </dict>
    <key>StandardOutPath</key>
    <string>${OUT_LOG}</string>
    <key>StandardErrorPath</key>
    <string>${ERR_LOG}</string>
  </dict>
</plist>
EOF
}

validate_rendered_plist() {
  local plist_file="$1"
  plutil -lint "${plist_file}" >/dev/null
}

install_plist() {
  require_cmd plutil
  require_trusted_bridge_identity
  mkdir -p "$(dirname "${PLIST_PATH}")" "${LOG_DIR}"

  local tmp_file timestamp
  tmp_file="$(mktemp /tmp/iterate-bridge-daemon.XXXXXX.plist)"
  render_plist >"${tmp_file}"
  validate_rendered_plist "${tmp_file}"

  if [ -f "${PLIST_PATH}" ]; then
    timestamp="$(date '+%Y%m%d-%H%M%S')"
    cp "${PLIST_PATH}" "${PLIST_PATH}.bak-${timestamp}"
    log "backed up existing plist to ${PLIST_PATH}.bak-${timestamp}"
  fi

  install -m 644 "${tmp_file}" "${PLIST_PATH}"
  rm -f "${tmp_file}"
  log "installed plist: ${PLIST_PATH}"
  log "not loaded. Run: $0 load"
}

port_owner_pids() {
  lsof -nP -tiTCP:"${PORT}" -sTCP:LISTEN 2>/dev/null || true
}

port_owner_summary() {
  local pid
  while IFS= read -r pid; do
    [ -n "${pid}" ] || continue
    ps -p "${pid}" -o pid= -o command= 2>/dev/null || true
  done < <(port_owner_pids)
}

ensure_port_available_for_load() {
  local owners
  owners="$(port_owner_summary)"
  if [ -n "${owners}" ] && [ "${ALLOW_PORT_IN_USE:-0}" != "1" ]; then
    cat >&2 <<EOF
Port ${PORT} is already listening:
${owners}

Refusing to load ${LABEL}; a bridge daemon would fail and launchd could restart-loop.
Stop the current owner first, or set ALLOW_PORT_IN_USE=1 if you intentionally want to test the failure path.
EOF
    exit 2
  fi
}

load_plist() {
  require_cmd launchctl
  require_cmd lsof
  require_trusted_bridge_identity
  [ -f "${PLIST_PATH}" ] || die "plist not found: ${PLIST_PATH}. Run: $0 install"
  ensure_port_available_for_load
  launchctl bootstrap "${DOMAIN}" "${PLIST_PATH}" 2>/dev/null || true
  launchctl kickstart -k "${SERVICE}"
  log "loaded ${SERVICE}"
}

unload_plist() {
  require_cmd launchctl
  launchctl bootout "${SERVICE}" >/dev/null 2>&1 ||
    launchctl bootout "${DOMAIN}" "${PLIST_PATH}" >/dev/null 2>&1 ||
    true
  log "unloaded ${SERVICE}"
}

restart_plist() {
  # Validate before unloading so a bad candidate cannot turn a healthy Bridge
  # into downtime and then fail during load.
  require_trusted_bridge_identity
  unload_plist
  load_plist
}

uninstall_plist() {
  unload_plist
  if [ -f "${PLIST_PATH}" ]; then
    rm -f "${PLIST_PATH}"
    log "removed plist: ${PLIST_PATH}"
  else
    log "plist already absent: ${PLIST_PATH}"
  fi
}

http_health() {
  curl --noproxy '*' -fsS --max-time 3 "http://127.0.0.1:${PORT}/api/version"
}

ws_health() {
  local header_file err_file status_line
  header_file="$(mktemp /tmp/iterate-bridge-ws-header.XXXXXX)"
  err_file="$(mktemp /tmp/iterate-bridge-ws-err.XXXXXX)"
  set +e
  curl --noproxy '*' -m 3 --http1.1 -i -sS -N \
    -H 'Connection: Upgrade' \
    -H 'Upgrade: websocket' \
    -H 'Sec-WebSocket-Version: 13' \
    -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' \
    "http://127.0.0.1:${PORT}/ws" >"${header_file}" 2>"${err_file}"
  set -e
  status_line="$(sed -n '1p' "${header_file}" | tr -d '\r')"
  rm -f "${header_file}" "${err_file}"

  printf '%s\n' "${status_line}"
  case "${status_line}" in
    *" 101 "*) return 0 ;;
    *) return 1 ;;
  esac
}

health() {
  require_cmd curl
  log "HTTP http://127.0.0.1:${PORT}/api/version"
  http_health
  printf '\n'
  log "WS http://127.0.0.1:${PORT}/ws"
  ws_health
}

status() {
  require_cmd launchctl
  require_cmd lsof
  printf 'Label: %s\n' "${LABEL}"
  printf 'Service: %s\n' "${SERVICE}"
  printf 'Plist: %s (%s)\n' "${PLIST_PATH}" "$([ -f "${PLIST_PATH}" ] && echo present || echo absent)"
  printf 'App: %s (%s)\n' "${APP_BIN}" "$([ -x "${APP_BIN}" ] && echo executable || echo missing)"
  printf 'Port: %s\n' "${PORT}"
  printf '\nlaunchctl:\n'
  if launchctl print "${SERVICE}" >/tmp/iterate-bridge-launchctl-status.$$ 2>&1; then
    sed -n '1,40p' /tmp/iterate-bridge-launchctl-status.$$
  else
    sed -n '1,12p' /tmp/iterate-bridge-launchctl-status.$$
  fi
  rm -f /tmp/iterate-bridge-launchctl-status.$$

  printf '\nport owner:\n'
  port_owner_summary || true

  printf '\nhealth:\n'
  if http_health >/tmp/iterate-bridge-http-health.$$ 2>/tmp/iterate-bridge-http-health-err.$$; then
    printf 'http: ok %s\n' "$(cat /tmp/iterate-bridge-http-health.$$)"
  else
    printf 'http: failed %s\n' "$(cat /tmp/iterate-bridge-http-health-err.$$)"
  fi
  rm -f /tmp/iterate-bridge-http-health.$$ /tmp/iterate-bridge-http-health-err.$$
  if ws_line="$(ws_health 2>/dev/null)"; then
    printf 'ws: ok %s\n' "${ws_line}"
  else
    printf 'ws: failed %s\n' "${ws_line:-}"
  fi
}

doctor() {
  require_cmd launchctl
  require_cmd lsof
  require_cmd curl
  require_cmd plutil
  printf 'preflight:\n'
  printf '  app executable: '
  [ -x "${APP_BIN}" ] && printf 'ok\n' || printf 'missing: %s\n' "${APP_BIN}"
  printf '  bridge code identity: '
  if trusted_bridge_identity; then
    printf 'ok\n'
  else
    printf 'invalid (requires Developer ID Team UM3Z9G5DNH and identifier com.kexin94yyds.iterate)\n'
  fi
  printf '  app supports --bridge-only: '
  local app_help
  if [ -x "${APP_BIN}" ] && app_help="$("${APP_BIN}" --help 2>/dev/null)" && grep -q -- '--bridge-only' <<<"${app_help}"; then
    printf 'ok\n'
  else
    printf 'unknown\n'
  fi
  printf '  rendered plist: '
  local tmp_file
  tmp_file="$(mktemp /tmp/iterate-bridge-daemon.XXXXXX.plist)"
  render_plist >"${tmp_file}"
  if plutil -lint "${tmp_file}" >/dev/null 2>&1; then
    printf 'ok\n'
  else
    printf 'invalid\n'
  fi
  rm -f "${tmp_file}"
  printf '  port owner:\n'
  port_owner_summary | sed 's/^/    /' || true
  if [ -f "${HOME}/Library/LaunchAgents/com.imhuso.iterate.bridge.plist.disabled" ]; then
    printf '  old disabled bridge plist: present (not bridge-only)\n'
  fi
}

command="${1:-status}"
case "${command}" in
  status) status ;;
  doctor) doctor ;;
  render) render_plist ;;
  install) install_plist ;;
  load) load_plist ;;
  unload) unload_plist ;;
  restart) restart_plist ;;
  uninstall) uninstall_plist ;;
  health) health ;;
  -h|--help|help) usage ;;
  *) usage >&2; exit 64 ;;
esac
