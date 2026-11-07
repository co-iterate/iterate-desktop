---
name: ji
description: 把刚解决的问题沉淀进 iterate 知识库（problems / regressions / patterns 三件套）。触发词：ji、ji0、ji1、沉淀、记一下、三件套、settle。
---

# ji：经验沉淀

把当前对话里刚解决的问题写进知识库，下次用 `xi` 能召回。

知识库根目录：环境变量 `CUNZHI_KNOWLEDGE_DIR`，未设置时为 `~/.cunzhi-knowledge`。下文用 `<KB>` 表示。

## 触发词

| 词 | 动作 |
|---|---|
| `ji0` | 只记问题：追加到 `<KB>/experience/problems.md` |
| `ji1` | 三件套：problems + regressions + patterns |
| `ji` | 先问用户要 ji0 还是 ji1（有 iterate 的 `call_zhi` 就用弹窗选项） |

## 执行步骤

1. **从当前对话提取**：现象、证据、根因、修复、如何验证。上下文不够时先用 `xi` 搜一下已有条目，避免重复。
2. **分配编号**：读目标文件里已有的最大编号，按 `P-YYYY-NNN` / `R-YYYY-NNN` / `PAT-YYYY-NNN` 递增（YYYY 用今年）。
3. **按模板追加**（只追加，不改写历史条目）：
   - `<KB>/templates/problem_entry.md` → `<KB>/experience/problems.md`
   - `<KB>/templates/regression_entry.md` → `<KB>/experience/regressions.md`（ji1）
   - `<KB>/templates/pattern_entry.md` → `<KB>/experience/patterns.md`（ji1）
4. **对应关系**：R 条目必须引用对应的 P 编号；PAT 只写能跨项目复用的做法，写不出来就跳过 PAT 并说明。
5. **可选 Git**：如果 `<KB>` 是 git 仓库，提交 `experience/`；有远端且用户同意时再推送。不是仓库就跳过，不要擅自 `git init`。
6. **汇报**：列出写入的编号和文件。

## 规则

- 事实、推断分开写；没验证的根因标 `待验证`，状态保持 `open`。
- 不写密码、token、私钥、个人隐私、只对本机有效的绝对路径。
- 不删除、不批量重写已有条目。
