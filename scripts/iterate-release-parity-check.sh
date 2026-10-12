#!/usr/bin/env bash
set -euo pipefail

WORKSPACE_PATH="${WORKSPACE_PATH:-${HOME}/cunzhi}"
APP_PATH="${APP_PATH:-/Applications/iterate.app}"
PUBLIC_RELEASES_PAGE_URL="${PUBLIC_RELEASES_PAGE_URL:-https://github.com/kexin94yyds/iterate-releases/releases/latest}"
PRIMARY_RELEASES_PAGE_URL="${PRIMARY_RELEASES_PAGE_URL:-https://github.com/kexin94yyds/iterate/releases/latest}"
README_RELEASES_URL="${README_RELEASES_URL:-https://github.com/kexin94yyds/iterate-releases/releases}"
CURL_CONNECT_TIMEOUT_SECONDS="${CURL_CONNECT_TIMEOUT_SECONDS:-10}"
CURL_MAX_TIME_SECONDS="${CURL_MAX_TIME_SECONDS:-30}"
RUN_ID="${RUN_ID:-$(date '+%Y-%m-%dT%H-%M-%S')_release_parity}"
RUN_ROOT="${RUN_ROOT:-${WORKSPACE_PATH}/.cunzhi-memory/release-parity-runs}"
RUN_DIR="${RUN_DIR:-${RUN_ROOT}/${RUN_ID}}"
SAMPLES_FILE="${RUN_DIR}/samples.jsonl"
SUMMARY_JSON="${RUN_DIR}/summary.json"
SUMMARY_MD="${RUN_DIR}/summary.md"
STDOUT_LOG="${RUN_DIR}/stdout.log"
STDERR_LOG="${RUN_DIR}/stderr.log"
STRICT="${STRICT:-0}"

mkdir -p "${RUN_DIR}"
: >"${SAMPLES_FILE}"
: >"${STDOUT_LOG}"
: >"${STDERR_LOG}"
exec > >(tee -a "${STDOUT_LOG}") 2> >(tee -a "${STDERR_LOG}" >&2)

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1" >&2
    exit 1
  fi
}

require_cmd curl
require_cmd git
require_cmd jq

cd "${WORKSPACE_PATH}"

json_string() {
  jq -Rn --arg value "$1" '$value'
}

semver_tag() {
  local version="$1"
  if [[ -z "${version}" || "${version}" == "null" ]]; then
    printf ''
  else
    printf 'v%s' "${version}"
  fi
}

http_head_json() {
  local name="$1"
  local url="$2"
  local header_file err_file metrics curl_status http_code effective_url total_time error_text

  header_file="$(mktemp /tmp/iterate-release-head.XXXXXX)"
  err_file="$(mktemp /tmp/iterate-release-head-err.XXXXXX)"
  set +e
  metrics="$(curl -sS -I -L \
    --connect-timeout "${CURL_CONNECT_TIMEOUT_SECONDS}" \
    --max-time "${CURL_MAX_TIME_SECONDS}" \
    -o "${header_file}" \
    -w '%{http_code} %{url_effective} %{time_total}' \
    "${url}" 2>"${err_file}")"
  curl_status="$?"
  set -e
  http_code="$(printf '%s' "${metrics}" | awk '{print $1}')"
  effective_url="$(printf '%s' "${metrics}" | awk '{print $2}')"
  total_time="$(printf '%s' "${metrics}" | awk '{print $3}')"
  error_text="$(cat "${err_file}" 2>/dev/null || true)"
  rm -f "${header_file}" "${err_file}"

  jq -cn \
    --arg name "${name}" \
    --arg url "${url}" \
    --arg curl_status "${curl_status}" \
    --arg http_code "${http_code:-0}" \
    --arg effective_url "${effective_url}" \
    --arg total_time "${total_time:-0}" \
    --arg error "${error_text}" \
    '{
      name: $name,
      url: $url,
      curl_status: ($curl_status | tonumber),
      http_code: ($http_code | tonumber? // 0),
      effective_url: $effective_url,
      total_time: ($total_time | tonumber? // 0),
      ok: (($curl_status | tonumber) == 0 and (($http_code | tonumber? // 0) >= 200 and (($http_code | tonumber? // 0) < 400))),
      error: $error
    }'
}

file_json() {
  local name="$1"
  local path="$2"
  local exists=false size="" sha256=""
  if [[ -f "${path}" ]]; then
    exists=true
    size="$(stat -f%z "${path}" 2>/dev/null || stat -c%s "${path}" 2>/dev/null || true)"
    sha256="$(shasum -a 256 "${path}" 2>/dev/null | awk '{print $1}' || true)"
  fi
  jq -cn \
    --arg name "${name}" \
    --arg path "${path}" \
    --arg size "${size}" \
    --arg sha256 "${sha256}" \
    --argjson exists "${exists}" \
    '{name:$name,path:$path,exists:$exists,size:($size|tonumber? // null),sha256:(if $sha256 == "" then null else $sha256 end)}'
}

package_version="$(jq -r '.version // empty' package.json 2>/dev/null || true)"
cargo_version="$(sed -nE 's/^version[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/p' Cargo.toml | head -1 || true)"
app_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "${APP_PATH}/Contents/Info.plist" 2>/dev/null || true)"
expected_tag="$(semver_tag "${package_version}")"

local_tag_exists=false
if [[ -n "${expected_tag}" ]] && git rev-parse -q --verify "refs/tags/${expected_tag}" >/dev/null; then
  local_tag_exists=true
fi

remote_tags="$(git ls-remote --tags origin 2>/dev/null || true)"
remote_tag_exists=false
windows_marker_tag_exists=false
if [[ -n "${expected_tag}" ]] && printf '%s\n' "${remote_tags}" | grep -Fq "refs/tags/${expected_tag}"; then
  remote_tag_exists=true
fi
if printf '%s\n' "${remote_tags}" | grep -Fq "refs/tags/windows-package-latest"; then
  windows_marker_tag_exists=true
fi

readme_release_links="$(rg -o 'https://github.com/kexin94yyds/[^]) ]+/releases[^]) ]*' README.md 2>/dev/null | sort -u | jq -R . | jq -s .)"
frontend_release_urls="$(rg -n 'repos/kexin94yyds/[^/]+/releases|github.com/kexin94yyds/[^/]+/releases' src/frontend/composables/useVersionCheck.ts 2>/dev/null | jq -R . | jq -s .)"
rust_release_urls="$(rg -n 'repos/kexin94yyds/[^/]+/releases|github.com/kexin94yyds/[^/]+/releases' src/rust/ui/updater.rs 2>/dev/null | jq -R . | jq -s .)"

public_release_head="$(http_head_json "public_iterate_releases_latest" "${PUBLIC_RELEASES_PAGE_URL}")"
primary_release_head="$(http_head_json "primary_iterate_latest" "${PRIMARY_RELEASES_PAGE_URL}")"
readme_release_head="$(http_head_json "readme_release_url" "${README_RELEASES_URL}")"

delivery_dmg="$(file_json "delivery_macos_dmg" "target/release/delivery/macos/iterate_${package_version}_aarch64.dmg")"
delivery_zip="$(file_json "delivery_macos_zip" "target/release/delivery/macos/iterate.app.zip")"
bundle_dmg="$(file_json "bundle_macos_dmg" "target/release/bundle/dmg/iterate_${package_version}_aarch64.dmg")"

sample="$(
  jq -cn \
    --arg ts "$(date '+%Y-%m-%dT%H:%M:%S%z')" \
    --arg package_version "${package_version}" \
    --arg cargo_version "${cargo_version}" \
    --arg app_version "${app_version}" \
    --arg expected_tag "${expected_tag}" \
    --argjson local_tag_exists "${local_tag_exists}" \
    --argjson remote_tag_exists "${remote_tag_exists}" \
    --argjson windows_marker_tag_exists "${windows_marker_tag_exists}" \
    --argjson readme_release_links "${readme_release_links}" \
    --argjson frontend_release_urls "${frontend_release_urls}" \
    --argjson rust_release_urls "${rust_release_urls}" \
    --argjson public_release_head "${public_release_head}" \
    --argjson primary_release_head "${primary_release_head}" \
    --argjson readme_release_head "${readme_release_head}" \
    --argjson delivery_dmg "${delivery_dmg}" \
    --argjson delivery_zip "${delivery_zip}" \
    --argjson bundle_dmg "${bundle_dmg}" \
    '{
      ts: $ts,
      versions: {
        package: $package_version,
        cargo: $cargo_version,
        app: $app_version,
        expected_tag: $expected_tag
      },
      tags: {
        local_expected_tag_exists: $local_tag_exists,
        remote_expected_tag_exists: $remote_tag_exists,
        windows_marker_tag_exists: $windows_marker_tag_exists
      },
      release_sources: {
        readme_links: $readme_release_links,
        frontend: $frontend_release_urls,
        rust: $rust_release_urls
      },
      probes: {
        public_release_head: $public_release_head,
        primary_release_head: $primary_release_head,
        readme_release_head: $readme_release_head
      },
      artifacts: {
        delivery_dmg: $delivery_dmg,
        delivery_zip: $delivery_zip,
        bundle_dmg: $bundle_dmg
      }
    }'
)"
printf '%s\n' "${sample}" >>"${SAMPLES_FILE}"

jq '
  def contains_public_releases:
    tostring | test("kexin94yyds/iterate-releases");
  def contains_primary_releases:
    tostring | test("kexin94yyds/iterate/releases");

  . as $sample |
  {
    run_id: "'"${RUN_ID}"'",
    run_dir: "'"${RUN_DIR}"'",
    checked_at: $sample.ts,
    status: "unknown",
    checks: {
      version_alignment: (
        ($sample.versions.package != "")
        and ($sample.versions.package == $sample.versions.cargo)
        and ($sample.versions.package == $sample.versions.app)
      ),
      local_tag_exists: $sample.tags.local_expected_tag_exists,
      remote_tag_exists: $sample.tags.remote_expected_tag_exists,
      windows_marker_tag_exists: $sample.tags.windows_marker_tag_exists,
      public_release_page_ok: (
        $sample.probes.public_release_head.ok == true
        and ($sample.probes.public_release_head.effective_url | contains($sample.versions.expected_tag))
      ),
      readme_release_page_ok: ($sample.probes.readme_release_head.ok == true),
      readme_points_public_release: ($sample.release_sources.readme_links | contains_public_releases),
      frontend_points_public_release: ($sample.release_sources.frontend | contains_public_releases),
      rust_mentions_public_release: ($sample.release_sources.rust | contains_public_releases),
      rust_avoids_primary_release: (($sample.release_sources.rust | contains_primary_releases) | not),
      local_mac_artifacts_exist: (
        $sample.artifacts.delivery_dmg.exists == true
        and $sample.artifacts.delivery_zip.exists == true
        and $sample.artifacts.bundle_dmg.exists == true
      )
    },
    diagnostics: {
      primary_release_page_ok: ($sample.probes.primary_release_head.ok == true),
      rust_mentions_primary_release: ($sample.release_sources.rust | contains_primary_releases)
    },
    sample: $sample
  }
  | .failures = (
      .checks
      | to_entries
      | map(select(.value != true) | .key)
    )
  | .status = (if (.failures | length) == 0 then "passed" else "failed" end)
' "${SAMPLES_FILE}" >"${SUMMARY_JSON}"

cat >"${SUMMARY_MD}" <<EOF
## iterate release parity check

- run_id: \`${RUN_ID}\`
- run_dir: \`${RUN_DIR}\`
- status: \`$(jq -r '.status' "${SUMMARY_JSON}")\`
- version_alignment: \`$(jq -r '.checks.version_alignment' "${SUMMARY_JSON}")\`
- local_tag_exists: \`$(jq -r '.checks.local_tag_exists' "${SUMMARY_JSON}")\`
- remote_tag_exists: \`$(jq -r '.checks.remote_tag_exists' "${SUMMARY_JSON}")\`
- windows_marker_tag_exists: \`$(jq -r '.checks.windows_marker_tag_exists' "${SUMMARY_JSON}")\`
- public_release_page_ok: \`$(jq -r '.checks.public_release_page_ok' "${SUMMARY_JSON}")\`
- primary_release_page_ok: \`$(jq -r '.diagnostics.primary_release_page_ok' "${SUMMARY_JSON}")\`
- readme_release_page_ok: \`$(jq -r '.checks.readme_release_page_ok' "${SUMMARY_JSON}")\`
- readme_points_public_release: \`$(jq -r '.checks.readme_points_public_release' "${SUMMARY_JSON}")\`
- frontend_points_public_release: \`$(jq -r '.checks.frontend_points_public_release' "${SUMMARY_JSON}")\`
- rust_mentions_public_release: \`$(jq -r '.checks.rust_mentions_public_release' "${SUMMARY_JSON}")\`
- rust_avoids_primary_release: \`$(jq -r '.checks.rust_avoids_primary_release' "${SUMMARY_JSON}")\`
- local_mac_artifacts_exist: \`$(jq -r '.checks.local_mac_artifacts_exist' "${SUMMARY_JSON}")\`
- diagnostic_primary_release_page_ok: \`$(jq -r '.diagnostics.primary_release_page_ok' "${SUMMARY_JSON}")\`
- diagnostic_rust_mentions_primary_release: \`$(jq -r '.diagnostics.rust_mentions_primary_release' "${SUMMARY_JSON}")\`
- failures: \`$(jq -c '.failures' "${SUMMARY_JSON}")\`

Artifacts:

- samples: \`${SAMPLES_FILE}\`
- summary_json: \`${SUMMARY_JSON}\`
- stdout_log: \`${STDOUT_LOG}\`
- stderr_log: \`${STDERR_LOG}\`
EOF

cat "${SUMMARY_MD}"

if [[ "${STRICT}" == "1" ]] && [[ "$(jq -r '.status' "${SUMMARY_JSON}")" != "passed" ]]; then
  exit 2
fi
