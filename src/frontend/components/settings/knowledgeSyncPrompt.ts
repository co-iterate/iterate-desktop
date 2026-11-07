/**
 * 「在 GitHub 创建知识库」提示词：用户复制给自己的 AI 去配置，iterate 本身不碰 git 凭据，也不自动推送。
 */
export function buildKnowledgeSyncPrompt(root: string): string {
  return `请帮我把 iterate 知识库放到我自己的 GitHub 上，建成一个私有仓库，之后用 ji 沉淀的经验都能有版本记录。

知识库目录：${root}

要求：
1. 先检查这个目录是不是已经是 git 仓库、有没有 remote；已经配好就只告诉我现状，不要重复配置。
2. 仓库必须是【私有】的。conversations/ 里是我的原始对话，绝对不能放到公开仓库。
   - 有 gh：先 \`gh auth status\` 确认已登录，再 \`git init\` 并用 \`gh repo create <名字> --private --source . --push\`。
   - 没有 gh：一步步教我在 GitHub 网页上新建私有仓库，再 \`git remote add origin <地址>\`。
3. 第一次提交前，加一个 .gitignore 排除 .DS_Store 等系统文件，并把要提交的文件列表给我确认。
4. 不要把 token、密码写进任何文件或 remote URL。
5. 完成后告诉我仓库地址，并确认它是 Private。
`
}
