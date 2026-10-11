export function buildFormalRouteSetupPrompt() {
  return `请帮助这名真实用户在当前 Mac 上启用 iterate 的 iPhone 远程连接二维码。你是在配置当前安装的软件和 Cloudflare 网络，不是在修改 iterate 源码。

成功标准：
- 当前安装的 iterate 主程序与 Bridge 来自同一正式版本。
- Bridge 由当前安装的后台服务唯一持有；当前正式二维码链路使用官方 origin http://127.0.0.1:8080。
- 用户自己的 Cloudflare Named Tunnel 通过 HTTPS/WSS 指向这台 Mac 的 iterate Bridge。
- 公网端点通过 HTTPS、WebSocket、鉴权边界和当前安装身份验证。
- 验证完成后，使用 iterate 的安全命令登记不含秘密的正式路线。
- 用户回到 iterate 点击顶部二维码后，可以看到当前有效的配对二维码。

当前版本能力边界：
- 正式路线只支持 cloudflare_named_tunnel。
- 禁止使用 Quick Tunnel、随机 trycloudflare.com、Relay、LAN 或关闭鉴权来代替正式配置。
- 官方正式路线要求 Bridge 位于 127.0.0.1:8080；如果当前安装使用自定义端口，停止并报告当前版本不支持，不要自行改写 Tunnel origin 后宣称成功。

安全边界：
- 不修改 iterate 源码，不直接手改正式路线状态，不开放无关端口。
- 不向我索取 token、Cookie、私钥、证书、二维码、配对链接、Keychain 内容、Cloudflare 凭据或原始私人日志。
- 可以只检查凭据文件是否存在以及权限是否合理，禁止读取、打印、复制或上传凭据内容，包括 ~/.cloudflared 下的证书和凭据 JSON。
- 登录 Cloudflare、选择账号、Zone、域名或 hostname，创建或修改 Tunnel/DNS，安装软件，注册后台服务，使用管理员权限或中断现有服务前，必须说明动作、影响和回滚方式，并等待我明确确认。

第一阶段：只读发现
1. 定位当前已安装的 iterate 可执行文件、版本、CPU 架构和实际 Bridge 后台服务；不要假设 Homebrew、cloudflared 或 iterate 一定位于固定路径。
2. 运行该安装版的 --mobile-route-status 和 --mobile-route-verify，只保留结构化、脱敏的结果。
3. 核对 Bridge 的实际 owner、启动参数和版本。loopback HTTP 检查必须绕过系统 HTTP/HTTPS/SOCKS 代理，避免代理制造 localhost 假 502。
4. 检查官方 cloudflared 是否已安装、是否已有 iterate 专用 Named Tunnel/hostname、是否存在多个 supervisor 抢占同一路线。只报告脱敏状态、名称和公开 hostname。
5. 先向我报告：当前属于已配置且健康、已配置但故障、存在可采用路线，还是从未配置；准备复用、修复或新建哪些对象；是否需要登录、域名、DNS、安装、管理员权限或停机；以及精确回滚方式。

第二阶段：严格按状态分流
A. 如果 --mobile-route-status 显示 configured=true 且 healthy：
- 不修改 Cloudflare、DNS、Tunnel 或后台服务。
- 再次 verify 后，直接告诉我回到 iterate 打开二维码。

B. 如果 configured=true 但不健康：
- 只修复回执中的既有 transport、base_url 和唯一 owner。
- 禁止新建 Tunnel、hostname 或 DNS，禁止切换路线，禁止删除现有 DeviceAuth 或已配对设备。
- 任何写入、重启、账号或管理员动作仍须先确认；修复后再次运行 --mobile-route-verify。

C. 如果 configured=false：
- 如果发现属于本机且用途明确的既有 iterate Named Tunnel，先提出“验证后采用”方案，不重复创建。
- 如果只有其他项目的 Tunnel，禁止覆盖或挪用；需要新建 iterate 专用 Tunnel 时先让我确认。
- 如果缺少 cloudflared，说明官方安装方案、来源和回滚方式，确认后再安装。
- 如果需要 Cloudflare 登录、选择 Zone、域名或 hostname，让我本人通过官方交互完成，不要要求我把凭据粘贴进聊天。
- 如果我没有 Cloudflare 托管域名，停止并明确说明当前版本无法完成长期稳定的正式二维码公网路线，不得用临时通道替代。

第三阶段：经确认后执行最小配置
1. 只复用或创建一条我批准的 Cloudflare Named Tunnel。
2. 为我确认的 hostname 建立 DNS route，不修改无关记录。
3. 将该 hostname 的 HTTPS/WSS origin 指向已确认的官方 iterate Bridge 127.0.0.1:8080，并保留明确的非匹配 fallback。
4. 建立一个唯一、可在重启后恢复的 cloudflared 后台 owner；不要同时运行多个 supervisor。
5. 不修改无关代理、防火墙、其他应用、项目源码或已有 iPhone DeviceAuth。

第四阶段：验证与登记
1. 验证本机 /api/version、Bridge owner 和版本。
2. 验证公网 /.well-known/iterate/health 与 /api/version 为 HTTPS 200，并返回当前安装的 iterate 身份。
3. 验证公网 WebSocket 可达；未携带设备鉴权时应被安全拒绝，而不是匿名进入。
4. 验证公网 hostname 的端点身份确实属于当前这台 Mac。
5. 只有全部通过后，运行当前安装版：--mobile-route-register --transport cloudflare_named_tunnel --base-url "https://用户确认的hostname" --source ai_configured。
6. 再运行 --mobile-route-status 和 --mobile-route-verify。

最终只报告：
- 当前机器类型、iterate 版本和 Bridge owner 是否一致。
- 复用、修复或新建了什么，以及正式公开 hostname。
- HTTPS、WSS、鉴权边界、端点归属和重启恢复的脱敏结果。
- 正式路线是否登记成功、回滚步骤，以及下一步“回到 iterate 点击顶部二维码”。

如果任何必要授权、域名、正式端口、端点身份或安全登记命令不满足，保留现状并报告最少人工步骤；不得宣称二维码已经启用。`
}

export function buildFormalRouteRepairPrompt(baseUrl: string, code: string) {
  const safeBaseUrl = /^https:\/\/[a-z0-9.-]+(?::\d+)?$/i.test(baseUrl.trim())
    ? baseUrl.trim()
    : '已配置的正式 HTTPS 域名（请从 iterate 脱敏状态中读取）'
  const safeCode = String(code || 'formal_route_unhealthy').replace(/[^\w.:-]/g, '_').slice(0, 120)
  return `请只修复这台 Mac 上 iterate 已经配置的正式 iPhone 公网路线，不要创建或切换到另一条路线。

已配置地址：${safeBaseUrl}
脱敏错误码：${safeCode}

安全边界：
- 先运行已安装 iterate 的 --mobile-route-status 和 --mobile-route-verify，并只读检查本机 Bridge、现有 Named Tunnel 服务、HTTPS/WSS 与端点身份。
- 如果需要登录 Cloudflare、选择账号或域名、修改 DNS、授予管理员权限或提供任何凭据，必须暂停，让我本人完成。
- 禁止索取 token、密钥、Cookie、二维码或配对链接；禁止读取、打印、复制或上传 ~/.cloudflared 内任何凭据内容。
- 禁止使用 Quick Tunnel；禁止使用 Relay；禁止删除现有正式配置、降低鉴权或另建临时公网地址。
- 只允许做与现有 ${safeBaseUrl} 直接相关、可回滚的本机恢复；云端或管理员操作必须先说明并等待我确认。

完成后再次运行 --mobile-route-verify，只报告根因、实际恢复动作、脱敏健康状态和回滚方式。若仍失败，保留现有配置并报告最少的人工步骤。`
}
