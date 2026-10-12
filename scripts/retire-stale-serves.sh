#!/usr/bin/env bash
# 安装新版后，让仍在执行旧二进制、且当前空闲的 `iterate --serve` 退出（P-2026-9354）。
# 旧 serve 的命令行仍显示 /Applications/iterate.app，但实际执行的是被挪进 .iterate-retired 的旧文件；
# 退役包一旦删除，它们过不了 bridge 的代码签名校验，APNs 推送会全部失败。
# 忙碌中的 serve 不动（下次再处理）；下一次 zhi 会由新版重新拉起 serve。
# 用法：retire-stale-serves.sh [installed-app]   环境：CUNZHI_RETIRE_SERVES_DRY_RUN=1 只打印
set -uo pipefail

installed_app="${1:-/Applications/iterate.app}"
installed_bin="${installed_app}/Contents/MacOS/iterate"
dry_run="${CUNZHI_RETIRE_SERVES_DRY_RUN:-0}"
[[ -x "${installed_bin}" ]] || { echo "retire-serves: 找不到 ${installed_bin}，跳过" >&2; exit 0; }
installed_inode="$(stat -f %i "${installed_bin}")"

for pid in $(pgrep -f "MacOS/iterate --serve" || true); do
  exe="$(lsof -a -p "${pid}" -d txt -Fn 2>/dev/null | sed -n 's/^n//p' | grep '/Contents/MacOS/iterate$' | head -n 1)"
  [[ -n "${exe}" ]] || continue
  exe_inode="$(stat -f %i "${exe}" 2>/dev/null || echo missing)"
  [[ "${exe_inode}" == "${installed_inode}" ]] && continue
  port="$(ps -o command= -p "${pid}" | sed -nE 's/.*--port ([0-9]+).*/\1/p')"
  busy="unknown"
  if [[ -n "${port}" ]]; then
    busy="$(curl -s -m 2 "http://127.0.0.1:${port}/status" \
      | python3 -c 'import json,sys; print(json.load(sys.stdin).get("is_busy"))' 2>/dev/null || echo unknown)"
  fi
  if [[ "${busy}" != "False" ]]; then
    echo "retire-serves: pid=${pid} port=${port:-?} 执行旧二进制但不空闲（busy=${busy}），本次保留"
    continue
  fi
  if [[ "${dry_run}" == "1" ]]; then
    echo "retire-serves: [dry-run] 将结束 pid=${pid} port=${port} exe=${exe}"
  else
    kill -TERM "${pid}" 2>/dev/null && echo "retire-serves: 已结束空闲旧 serve pid=${pid} port=${port} exe=${exe}"
    [[ -n "${port}" ]] && rm -f "${HOME}/.cunzhi_ports/${port}"
  fi
done
exit 0
