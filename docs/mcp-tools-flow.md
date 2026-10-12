# iterate MCP 工具完整流程文档

> 基于 `src/bin/mcp-server.rs` 实现 + 全局规则（00-bootstrap.md / 02-tools.md / 03-workflows.md）

---

## 工具总览

| 工具 | 触发词 | 功能 | 类型 |
|---|---|---|---|
| `call_zhi` | zhi | iterate GUI 弹窗 | 交互 |
| `sync` | sync | 同步 .cunzhi-knowledge | git |
| `checkpoint` | — | 项目 git 检查点 | git |
| `xi` | xi | 搜索历史经验 | 只读 |
| `ji` | ji | 回忆/记忆/沉淀/摘要 | 读写 |
| `ci` | ci | 搜索提示词库 | 只读 |
| `ask_smart_friend` | ask | Codex CLI 咨询 | 外部 |

---

## 1. call_zhi — iterate GUI 弹窗

### 参数
```
message: string (必填) — 显示给用户的内容（支持 Markdown）
project_path: string (可选) — 项目路径，用于 git 检查点
predefined_options: string[] (可选) — 预定义选项列表
is_markdown: boolean (可选，默认 true)
```

### 流程
```
AI 调用 call_zhi(message="...", predefined_options=[...])
  ↓
智能检测可用端口（5311 开始，检查 /health + /status）
  ↓
POST http://127.0.0.1:{port}/api/dialog
  ↓
iterate GUI 弹出，显示 message 和选项
  ↓
用户输入/选择
  ↓
返回：用户输入 / 选中的选项 / 附加图片 / 继续对话: true/false
  ↓
[Hook] 记录对话到 .cunzhi-knowledge/conversations/YYYY-MM-DD/{project}.md
```

### 返回格式
```
用户输入: {text}
选中的选项: {option1}, {option2}
附加图片: {path1}, {path2}
继续对话: true/false
```

---

## 2. sync — 同步知识库

### 参数
```
project_path: string (可选) — 用于定位 .cunzhi-knowledge
direction: "pull" | "push" | "both" (可选，默认 "pull")
```

### 流程（按 sync-knowledge/SKILL.md 规则）

```
sync()
  ↓
定位 .cunzhi-knowledge 目录
（优先 project_path/.cunzhi-knowledge → cwd → home）
  ↓
步骤 1：git status --porcelain（检查本地变更）
  ↓
步骤 2：有本地变更？
  是 → git add -A + git commit -m "sync: YYYY-MM-DD HH:MM"
  否 → "✅ 本地无变更"
  ↓
步骤 3（direction=pull 或 both）：
  git fetch --quiet
  git pull --no-rebase --quiet
  → 有更新："✅ 已拉取最新更新"
  → 已最新："✅ Already up to date"
  → 冲突："⚠️ Merge 冲突，需要手动解决"
  ↓
步骤 4（direction=push 或 both）：
  git push --quiet
  → 成功："🚀 已推送到 GitHub"
  → 失败："⚠️ git push 失败: {stderr}"
```

### 典型输出
```
## 🔄 知识库同步

目录: `/Users/example/.cunzhi-knowledge`

✅ 本地无变更
✅ Fetched from origin
✅ Already up to date
```

---

## 3. checkpoint — 项目 git 检查点

### 参数
```
project_path: string (必填) — 项目根目录（git 根）
message: string (可选) — 提交信息，默认 "checkpoint: YYYY-MM-DD HH:MM:SS"
```

### 流程
```
checkpoint(project_path="/Users/example/cunzhi")
  ↓
验证路径存在
  ↓
git add -A
  ↓
git status --porcelain（检查是否有改动）
  → 无改动："ℹ️ 没有未提交的改动，无需创建检查点"
  → 有改动 ↓
git commit -m "{message}" --quiet --no-verify
  ↓
git rev-parse --short HEAD（获取 commit hash）
  ↓
返回："✅ 检查点已创建 - 提交: `{hash}` - 信息: {message}"
```

---

## 4. xi — 搜索历史经验

### 参数
```
query: string (必填) — 搜索关键词
project_path: string (可选)
```

### 流程
```
xi(query="git push 失败")
  ↓
定位 .cunzhi-knowledge 目录
  ↓
并行搜索三个文件（按 ## 段落分割，关键词匹配）：
  patterns.md  → "📘 最佳实践"
  problems.md  → "🐛 问题记录"
  regressions.md → "🔄 回归经验"
  ↓
每个文件最多返回 5 个匹配段落（前 10 行 + 截断标记）
  ↓
返回格式化结果
```

### 典型输出
```
# 🔍 历史经验查找结果

查询：「git push 失败」

## 📘 最佳实践 (patterns.md)

## PAT-2025-001 git push 认证问题
...

---

## 🐛 问题记录 (problems.md)

## P-2025-003 git push 403 错误
...
```

---

## 5. ji — 回忆/记忆/沉淀/摘要

### 参数
```
action: "回忆" | "记忆" | "沉淀" | "摘要" (必填)
content: string (记忆/沉淀/摘要时必填)
category: "problems" | "regressions" | "patterns" (沉淀时必填)
project_path: string (必填，必须绑定 git 根目录)
```

---

### 5a. 回忆（读取知识库）

```
ji(action="回忆", project_path="...")
  ↓
定位 .cunzhi-knowledge 目录
  ↓
读取三件套文件（前 30 行 + 截断标记）：
  patterns.md / problems.md / regressions.md
  ↓
返回知识库内容摘要
```

---

### 5b. 记忆（写入 .cunzhi-memory/）

```
ji(action="记忆", content="...", project_path="...")
  ↓
验证 content 非空
  ↓
创建 {project_path}/.cunzhi-memory/ 目录（如不存在）
  ↓
追加写入 .cunzhi-memory/context.md
格式：
  ## YYYY-MM-DD HH:MM
  
  {content}
  ↓
返回："✅ 已写入记忆"
```

---

### 5c. 沉淀（写入 .cunzhi-knowledge/）

```
ji(action="沉淀", category="problems", content="P-2025-001 ...", project_path="...")
  ↓
验证 content 非空
  ↓
ID 格式校验：
  problems  → 必须包含 "P-"
  regressions → 必须包含 "R-"
  patterns  → 必须包含 "PAT-"
  → 不符合 → 报错拦截
  ↓
ID 冲突检测：
  提取内容中的 ID（如 P-2025-001）
  检查目标文件是否已有相同 ID
  → 有冲突 → iterate 弹窗警告，用户选择继续/取消
  ↓
patterns 特殊处理（规则强制）：
  iterate 弹窗预览内容，用户确认后才写入
  → 用户取消 → 中止，不写入
  ↓
追加写入 .cunzhi-knowledge/{category}.md
格式：
  ## YYYY-MM-DD HH:MM
  
  {content}
  ↓
自动 git 同步：
  git add {filename}
  git commit -m "auto: 沉淀 {filename} YYYY-MM-DD HH:MM"
  git push --quiet
  → 成功："🚀 已自动推送到 GitHub"
  → 失败："⚠️ git push 失败: {stderr}"
```

---

### 5d. 摘要（写入 conversations/）

```
ji(action="摘要", content="...", project_path="...")
  ↓
验证 content 非空
  ↓
定位 .cunzhi-knowledge 目录
  ↓
确定目标路径：
  .cunzhi-knowledge/conversations/YYYY-MM-DD/{project_name}.md
  ↓
追加写入：
  ## HH:MM 摘要
  
  {content}
  ↓
返回："✅ 摘要已写入 {path}"
```

---

## 6. ci — 提示词库搜索

### 参数
```
directory: string (必填) — 目录名（如 ci、git、testing）
query: string (可选) — 搜索关键词，留空列出所有
project_path: string (可选)
```

### 流程
```
ci(directory="git", query="push")
  ↓
定位 .cunzhi-knowledge/prompts/{directory}/ 目录
  → 不存在 → 列出所有可用目录
  ↓
遍历目录中的 .md / .txt 文件
  → query 为空：列出所有文件（前 20 行）
  → query 非空：文件名或内容包含关键词
  ↓
最多返回 5 个匹配文件（前 20 行 + 截断标记）
```

---

## 7. ask_smart_friend — Codex CLI 咨询

### 参数
```
question: string (必填) — 问题/代码方案/Bug 描述
project_path: string (可选)
```

### 流程
```
ask_smart_friend(question="...", project_path="...")
  ↓
检测 codex 二进制路径：
  /opt/homebrew/bin/codex
  /usr/local/bin/codex
  /usr/bin/codex
  which codex
  → 未找到 → 报错
  ↓
执行：codex --approval-policy never --full-auto "{question}"
  工作目录：project_path 或 "."
  ↓
等待结果（同步阻塞）
  ↓
返回 stdout 或 stderr 内容
格式："## 智能助手的建议\n\n{output}"
```

---

## 工具目录结构

```
.cunzhi-knowledge/          ← 全局知识库（GitHub 同步）
├── problems.md             ← Bug 记录（P-YYYY-NNN）
├── regressions.md          ← 回归经验（R-YYYY-NNN）
├── patterns.md             ← 最佳实践（PAT-YYYY-NNN）
├── conversations/          ← 对话记录
│   └── YYYY-MM-DD/
│       └── {project}.md
└── prompts/                ← 提示词库
    ├── ci/
    ├── git/
    └── testing/

.cunzhi-memory/             ← 项目级临时记忆（本地）
├── context.md              ← ji(记忆) 写入
├── preferences.md
└── rules.md
```

---

## ID 格式规范

| 文件 | ID 格式 | 示例 |
|---|---|---|
| problems.md | P-YYYY-NNN | P-2025-001 |
| regressions.md | R-YYYY-NNN | R-2025-001 |
| patterns.md | PAT-YYYY-NNN | PAT-2025-001 |

**约束**：
- P-ID 与 R-ID 必须一一对应
- 禁止只写 problems.md 而跳过 regressions.md
- patterns 写入前必须 iterate 预览确认
- 发现 ID 冲突时必须 iterate 确认是否重编号
