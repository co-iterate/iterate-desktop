# iterate 快速排查指南

这份文档只覆盖当前主方案下最常见的排查路径。

如果你是普通用户，优先看：
- [docs/iterate_安装指南.md](docs/iterate_安装指南.md)
- 安装包里的 `INSTALLATION.md`

如果你正在调试客户端接入、发布物或本地构建，再继续看下面。

## 1. 先确认客户端配置是否走的是当前口径

当前主入口统一为：
- 服务名：`iterate-zhi`
- 可执行文件：`mcp-server`
- `args`: `[]`

### Windsurf / Cursor

```json
{
  "mcpServers": {
    "iterate-zhi": {
      "command": "/path/to/mcp-server",
      "args": []
    }
  }
}
```

### Codex CLI

```toml
[mcp_servers."iterate-zhi"]
command = "/path/to/mcp-server"
args = []
disabled = false
enabled = true
tool_timeout_sec = 315360000
```

如果你用的是 Codex CLI，建议把 `tool_timeout_sec = 315360000` 一并保留，避免 `call_zhi` 这类交互工具在外层宿主上被过早超时。

如果你还看到下面这些旧口径，优先先清掉再排查：
- `cunzhi`
- `iterate`
- `iterate-xin`
- 固定端口参数（例如 `["5311"]`）
- 指向旧二进制名的配置

## 2. 确认 `mcp-server` 实际存在

```bash
ls -l /path/to/mcp-server
```

如果是安装包安装，优先以安装脚本最终落地的路径为准，不要沿用旧教程里的手写路径。

## 3. 修改配置后重启客户端

配置文件更新后，客户端通常需要重启或手动重连 MCP，新的配置才会生效。

## 4. 确认发布物本身没有缺料

当前对外交付的安装包至少应包含：
- `iterate`
- `mcp-server`
- `install.sh`（macOS / Linux）
- `INSTALLATION.md`

如果包里没有这些内容，或者说明书还在要求用户手工拼旧配置，这属于发布物问题。

## 5. 开发场景下的最小检查

如果你改的是安装链路、前端提示词或发布材料，建议至少跑：

```bash
bash -n install.sh release-package/install.sh
git diff --check
pnpm exec eslint src/frontend/components/tabs/PromptsTab.vue
```

## 6. 如果问题还在

继续看这些文档：
- [TROUBLESHOOTING.md](../TROUBLESHOOTING.md)
- [MCP_INSTALL.md](../MCP_INSTALL.md)
- [MCP_SERVER.md](../MCP_SERVER.md)
