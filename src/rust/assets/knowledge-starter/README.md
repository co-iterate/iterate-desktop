# iterate 知识库

位置：`~/.cunzhi-knowledge`。这是 iterate 第一次连接 AI 客户端时为你生成的本地知识库，之后完全归你所有：随便改，iterate 不会覆盖。

## 三个命令

在 Claude Code / Codex 里直接说：

| 命令 | 作用 |
|---|---|
| `hui` | 从 iterate 自动记录的对话里恢复"上次做到哪" |
| `xi` | 动手前查以前踩过的坑 |
| `ji` / `ji1` | 把刚解决的问题沉淀进三件套 |

这些 Skill 在 `prompts/skills/` 下，iterate 已把它们链接到 `~/.claude/skills/` 和 `~/.codex/skills/`。要改行为，直接改这里的 `SKILL.md`。

## 结构

```text
.cunzhi-knowledge/
├── README.md
├── AGENTS.md                 写入约定
├── experience/               ← ji 写入，xi 读取
│   ├── problems.md           P-YYYY-NNN 问题事实
│   ├── regressions.md        R-YYYY-NNN 回归检查
│   └── patterns.md           PAT-YYYY-NNN 可复用模式
├── templates/                三件套条目格式
├── conversations/            ← iterate 自动写入，hui 读取
└── prompts/skills/
    ├── INDEX.md
    ├── ji/SKILL.md
    ├── hui/SKILL.md
    └── xi/SKILL.md
```

## 换位置

设置环境变量 `CUNZHI_KNOWLEDGE_DIR` 指向别的目录即可，iterate 和三个 Skill 都会跟着用。

## 备份

建议在这个目录里 `git init` 并推到你自己的**私有**仓库。`conversations/` 里是原始对话，不要公开。
