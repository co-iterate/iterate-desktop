# shellcheck shell=bash
# 托管服务地址不写进源码：构建时从环境变量或仓库外的配置文件读取，
# 由 src/rust/tunnel/managed.rs 的 option_env! 在编译期注入。
#
# 用法：source scripts/managed-build-env.sh && load_managed_build_env [--require]
#   --require  未配置时报错返回 1（官方签名交付用）；默认只警告，社区构建不含托管连接。
# 配置文件：${CUNZHI_MANAGED_BUILD_ENV:-$HOME/.config/iterate/managed-build.env}
#   只解析下面两个键（KEY=value），不执行文件内容：
#   ITERATE_MANAGED_CONNECT_HOST=connect.example.com
#   ITERATE_MANAGED_DEVICE_DOMAIN=example.com

managed_build_env_value() {
  local file="$1"
  local key="$2"
  sed -n "s/^[[:space:]]*${key}[[:space:]]*=[[:space:]]*['\"]\{0,1\}\([a-z0-9.-]*\)['\"]\{0,1\}[[:space:]]*$/\1/p" "${file}" | tail -n 1
}

load_managed_build_env() {
  local require="${1:-}"
  local file="${CUNZHI_MANAGED_BUILD_ENV:-${HOME}/.config/iterate/managed-build.env}"

  if [[ -z "${ITERATE_MANAGED_CONNECT_HOST:-}" && -z "${ITERATE_MANAGED_DEVICE_DOMAIN:-}" && -f "${file}" ]]; then
    ITERATE_MANAGED_CONNECT_HOST="$(managed_build_env_value "${file}" ITERATE_MANAGED_CONNECT_HOST)"
    ITERATE_MANAGED_DEVICE_DOMAIN="$(managed_build_env_value "${file}" ITERATE_MANAGED_DEVICE_DOMAIN)"
  fi

  if [[ -n "${ITERATE_MANAGED_CONNECT_HOST:-}" && -n "${ITERATE_MANAGED_DEVICE_DOMAIN:-}" ]]; then
    export ITERATE_MANAGED_CONNECT_HOST ITERATE_MANAGED_DEVICE_DOMAIN
    return 0
  fi

  if [[ -n "${ITERATE_MANAGED_CONNECT_HOST:-}" || -n "${ITERATE_MANAGED_DEVICE_DOMAIN:-}" ]]; then
    echo "managed-build-env: ITERATE_MANAGED_CONNECT_HOST 与 ITERATE_MANAGED_DEVICE_DOMAIN 必须同时设置" >&2
    return 1
  fi

  unset ITERATE_MANAGED_CONNECT_HOST ITERATE_MANAGED_DEVICE_DOMAIN
  if [[ "${require}" == "--require" ]]; then
    echo "managed-build-env: 官方构建必须配置托管服务（环境变量或 ${file}）" >&2
    return 1
  fi
  echo "managed-build-env: 未配置托管服务，本次构建不含托管连接（手机连接将走自建设置）" >&2
  return 0
}
