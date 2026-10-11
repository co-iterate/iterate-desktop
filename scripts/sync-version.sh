#!/bin/zsh
# sync-version.sh - 一键同步所有版本号
# 用法：./scripts/sync-version.sh 0.5.3
# 同步目标：package.json、Cargo.toml、tauri.conf.json、version.json（release.yml 优先用它校验 tag）

set -euo pipefail

if [[ $# -lt 1 ]]; then
    # 无参数时显示当前版本
    echo "当前版本号:"
    echo "  package.json:    $(grep '"version"' package.json | head -1 | awk -F'"' '{print $4}')"
    echo "  Cargo.toml:      $(grep '^version' Cargo.toml | head -1 | awk -F'"' '{print $2}')"
    echo "  tauri.conf.json: $(grep '"version"' tauri.conf.json | head -1 | awk -F'"' '{print $4}')"
    [[ -f version.json ]] && echo "  version.json:    $(grep '"version"' version.json | head -1 | awk -F'"' '{print $4}')"
    echo ""
    echo "用法: $0 <新版本号>"
    echo "示例: $0 0.5.3"
    exit 0
fi

VERSION="$1"

# 验证版本号格式
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "❌ 版本号格式错误: $VERSION (应为 x.y.z)"
    exit 1
fi

echo "📦 同步版本号到 $VERSION"

# package.json
sed -i '' "s/\"version\": \"[^\"]*\"/\"version\": \"$VERSION\"/" package.json
echo "  ✅ package.json"

# Cargo.toml (只改第一个 version)
sed -i '' "0,/^version = \"[^\"]*\"/s//version = \"$VERSION\"/" Cargo.toml
echo "  ✅ Cargo.toml"

# tauri.conf.json
sed -i '' "s/\"version\": \"[^\"]*\"/\"version\": \"$VERSION\"/" tauri.conf.json
echo "  ✅ tauri.conf.json"

# version.json（release.yml 的版本校验优先读它）
if [[ -f version.json ]]; then
    sed -i '' "s/\"version\"[[:space:]]*:[[:space:]]*\"[^\"]*\"/\"version\": \"$VERSION\"/" version.json
    sed -i '' "s/\"build_date\"[[:space:]]*:[[:space:]]*\"[^\"]*\"/\"build_date\": \"$(date +%Y-%m-%d)\"/" version.json
    echo "  ✅ version.json"
fi

echo ""
echo "✅ 所有版本号已同步到 $VERSION"
echo "下一步: make save m=\"bump version to $VERSION\""
