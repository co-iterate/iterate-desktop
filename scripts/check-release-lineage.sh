#!/usr/bin/env bash
# 安装前的血缘检查（P-2026-9347）：待装 App 的源码必须同时包含
#   1) 发布主线（默认 release/desktop）的最新提交；
#   2) 当前已安装 App 的源码提交；
# 否则会静默覆盖掉别的会话已经装上的修复。
#
# 用法：check-release-lineage.sh <repo-root> <source-app> [installed-app]
# 环境：CUNZHI_RELEASE_BRANCH（默认 release/desktop）
#       CUNZHI_MACOS_ALLOW_LINEAGE_REGRESSION=1 明确知情时跳过（只告警）
set -euo pipefail

repo_root="${1:?repo root required}"
source_app="${2:?source app required}"
installed_app="${3:-/Applications/iterate.app}"
release_branch="${CUNZHI_RELEASE_BRANCH:-release/desktop}"
receipt_rel="Contents/Resources/cunzhi-source-receipt.json"

receipt_commit() {
  local receipt="$1/${receipt_rel}"
  [[ -f "${receipt}" ]] || return 1
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("source_commit") or "")' "${receipt}"
}

fail() {
  echo "lineage: $*" >&2
  if [[ "${CUNZHI_MACOS_ALLOW_LINEAGE_REGRESSION:-}" == "1" ]]; then
    echo "lineage: CUNZHI_MACOS_ALLOW_LINEAGE_REGRESSION=1，按用户明确要求继续安装" >&2
    exit 0
  fi
  echo "lineage: 先把缺的提交合进待装分支再构建；确需覆盖时设置 CUNZHI_MACOS_ALLOW_LINEAGE_REGRESSION=1" >&2
  exit 3
}

source_commit="$(receipt_commit "${source_app}")" || fail "待装 App 缺少源码收据：${source_app}/${receipt_rel}"
[[ -n "${source_commit}" ]] || fail "待装 App 的源码收据没有 source_commit"
git -C "${repo_root}" cat-file -e "${source_commit}^{commit}" 2>/dev/null \
  || fail "本地仓库找不到待装提交 ${source_commit}"

# 本地分支可能落后于远端（例如一直用 push HEAD:<branch> 推送），两者都要比。
mainline_found=0
for ref in "${release_branch}" "origin/${release_branch}"; do
  git -C "${repo_root}" rev-parse --verify --quiet "${ref}^{commit}" >/dev/null || continue
  mainline_found=1
  release_commit="$(git -C "${repo_root}" rev-parse "${ref}")"
  git -C "${repo_root}" merge-base --is-ancestor "${release_commit}" "${source_commit}" \
    || fail "待装 ${source_commit:0:9} 不包含 ${ref} 最新提交 ${release_commit:0:9}"
done
if [[ "${mainline_found}" -eq 0 ]]; then
  echo "lineage: 找不到发布主线 ${release_branch}，跳过主线检查" >&2
fi

if installed_commit="$(receipt_commit "${installed_app}")" && [[ -n "${installed_commit}" ]]; then
  git -C "${repo_root}" cat-file -e "${installed_commit}^{commit}" 2>/dev/null \
    || fail "本地仓库找不到已安装提交 ${installed_commit:0:9}（先 git fetch）"
  git -C "${repo_root}" merge-base --is-ancestor "${installed_commit}" "${source_commit}" \
    || fail "待装 ${source_commit:0:9} 不包含已安装的 ${installed_commit:0:9}，安装会覆盖掉已装修复"
else
  echo "lineage: 已安装 App 没有可用的源码收据，跳过已安装检查" >&2
fi

echo "lineage: ok（待装 ${source_commit:0:9} 包含 ${release_branch} 与已安装版本）"
