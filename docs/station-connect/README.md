# 站点连接握手（connect.html）

让用户在**自己的默认浏览器**里完成中转站登录后，把登录态接力给 LoongPort 桌面应用。
背景与设计动机见文末「为什么需要这个页面」。

站长接入只需要做一件事：**把这个文件挂到自己站点的域名下**。

## 一键配置（推荐）

能 SSH 登录服务器的话，一条命令完成（脚本事实源在
LoongPort-website 仓 `public/connect/setup.sh`）：

```sh
curl -fsSL https://loongport.dev/connect/setup.sh | sudo sh -s -- <你的域名>
```

- 自动识别 宝塔 / 1Panel / nginx / Caddy，只写一个页面文件
  （`/opt/loongport/connect.html`）和一段带标记的反代配置；
- 改配置前自动备份（集中放 `/opt/loongport/backups`，不会污染配置目录），
  `nginx -t` / `caddy validate` 通过才重载，失败自动还原；
- 完成后自检 `https://<域名>/.well-known/loongport/connect`；
- 再跑一遍就是更新；`sh setup.sh <你的域名> --remove` 完全移除。

识别不了的环境（如反代跑在容器里、路径非标）会给出手动粘贴指引，
配好后再跑一次命令即完成自检。手动步骤见下。

## 接入三步（手动）

1. 下载本目录的 [`connect.html`](./connect.html)（国内访问 GitHub 不稳时可直接取官网副本：
   `curl -o connect.html https://loongport.dev/connect/`——文件相同，事实源以本目录为准；
   地址须带尾斜杠（无斜杠会被托管层 308）。用面板的文件管理器上传同理，
   放哪个目录都行，下一步的路径对齐即可）；
2. 按部署方式挂载到同源路径（见下节），推荐约定路径
   `/.well-known/loongport/connect`（任意路径也可用，只是约定路径可以让客户端
   自动发现）；
3. 完事。用户在浏览器登录站点后打开该页面，点「打开 LoongPort」即完成接力。

## 按部署方式挂载

- **宝塔 / 1Panel 等面板**：在站点的「配置文件 / 伪静态」编辑处，把 nginx 片段
  贴进该域名的 `server { }` 里；`alias` 指到文件实际位置（宝塔默认站点根为
  `/www/wwwroot/<站点目录名>/connect.html`）。
- **手管 nginx**：片段加进该域名的 `server { }`：

```nginx
location = /.well-known/loongport/connect {
    default_type text/html;
    alias /opt/your-site/connect.html;   # 指向第一步下载的文件
}
# 「=」是精确匹配，保留原样即可
```

- **Caddy**：加进该域名的站点块里（与已有的 reverse_proxy 并列）；该形状已在
  `docker caddy:2-alpine` 实测通过（200 + `text/html` + 正确内容）：

```caddy
handle /.well-known/loongport/connect {
    root * /opt/your-site
    rewrite * /connect.html
    file_server
}
```

> 静态托管/对象存储同理：能以自己域名的一个 URL 提供 `text/html` 即可。
> 页面无任何外部依赖、无构建步骤、不向任何服务器发请求。

## 契约规格

**页面 URL**：`https://<站点>/.well-known/loongport/connect?state=<nonce>`

- `state`：可选。由客户端发起接力时生成，页面原样透传，客户端用它把回调绑定到
  自己发起的那次流程。

**回调**（页面在用户点击按钮后发起的重定向）：

```
loongport://connect
    ?origin=<页面自己的 origin>
    &state=<透传>
    &kind=<sub2api | newapi-session | newapi-access-token>
    &token=<登录凭据>
    &expires_at=<可选，sub2api 毫秒时间戳字符串，原样透传>
    &user_id=<可选，newapi-session 时由页面带上>
```

页面按站点家族自动检测凭据形状（按顺序尝试，命中即止）：

| kind | 家族/版本 | 凭据来源 |
|---|---|---|
| `sub2api` | sub2api | `localStorage.auth_token`（键名事实源：sub2api `frontend/src/stores/auth.ts`） |
| `newapi-session` | 旧版 new-api / one-api 形态 | 会话 cookie `session`（仅当未设 `HttpOnly` 时可读） |
| `newapi-access-token` | 旧版 new-api | `localStorage.user` JSON 里的 `access_token`（one-api 血统的系统访问令牌） |
| `newapi-access-token` | **现代版 new-api**（最新版真机实测） | 检测只认**非 httpOnly 的登录标志 cookie** `new_api_has_session=1`（零网络请求）；token 在点击「打开 LoongPort」时经同源 `POST /api/user/auth/refresh` **现铸一次** |

现代版 new-api 的形状与边界（自建最新版实测，2026-09）：

- 前端 localStorage **不存任何凭据**，会话在 httpOnly cookie 里，JWT 只在页面内存；
- `GET /api/user/token`（铸独立系统令牌的老路）被安全验证门拦（403
  `SECURITY_PROOF_REQUIRED`），页面走不通；
- `POST /api/user/auth/refresh` 可以铸 token，但**每次调用都会轮换会话 cookie**，
  短时间内多次铸会触发站点的 reuse 判定、把整个会话族连坐失效——所以检测用
  标志 cookie、铸币只在点击时**恰好一次**，绝不轮询着铸；
- 铸出的 token 有效期约 15 分钟（`expires_at` 是**秒**，消费端按「>1e11 才是
  毫秒」归一）。接力产物是短期会话——但足够客户端完成档位预配（sk 长期有效）。

`user_id` 只服务 `newapi-session`：客户端要拿会话 cookie 打 new-api 的
`GET /api/user/token` 换一把 Bearer 访问令牌，该端点要求 `New-Api-User` 头，
页面从 `localStorage.user.id` 读出随回调带上。`sub2api` 与现代版
`newapi-access-token` 不需要它。

用户资料（昵称等）**不在契约里**：客户端拿到凭据后自己调站点的 profile 端点
获取，避免经手多余的个人数据（`user_id` 例外——它不是资料，是换令牌的调用参数）。

## 安全模型

- **HTTPS-only**：页面检测到非 HTTPS（本机调试除外）直接拒绝工作。
- **显式确认**：移交必须由用户点击按钮触发（同时满足浏览器对自定义 scheme
  跳转的 user activation 要求）；页面不自动重定向。
- **state 绑定**：客户端发起时生成 nonce、回调时校验，防止其它页面凭空塞凭据。
- **不移交 refresh token**。sub2api 的 refresh token 是一次性轮换设计，浏览器的
  会话与客户端的会话不能共享同一把——移交等于让用户浏览器里的登录态静默失效。
  因此接力产物是**短期会话**（access token 到期后需要重新接力或改用客户端内
  登录）。这是有意为之的边界，不是遗漏。
- 凭据只出现在「页面 → 本机应用」的一次重定向里，不落任何中间服务器。

## 登录态缺失或过期时

页面不会替用户跳走（也不自动移交任何东西），给的是引导闭环：

1. 未检测到登录态 / sub2api 登录态已过期（页面读 `token_expires_at` 预判，
   过期凭据不移交——移交给客户端也只会被 profile 验证打回）；
2. 点「在新标签页登录或注册本站账号」（`/login`，站内自行跳注册同样有效——
   注册成功即自动登录）——**新标签**保证本页不丢，
   页面以 1 秒轮询盯着本站凭据；
3. 登录完成回到本页，**自动就绪，无需刷新**；点「打开 LoongPort」完成接力。

最后一步的点击无法省掉：浏览器只允许由用户手势触发自定义 scheme 跳转
（也是「本机应用被唤起必须用户知情」的安全边界），这属于有意设计。

## 已知边界

- **new-api 已两代兼容**（旧版 cookie/localStorage 分支 + 现代版标志 cookie +
  点击现铸，均真机验证）；再往后的可靠解仍是上游契约（见下）。
- **客户端接收器已落地**：LoongPort 收到回调后拿凭据打一次站点 profile 验证
  （打不通拒收、不落行），成功即走与登录窗相同的落库链（合并/去重语义一致），
  toast + 自动预配档位。未安装应用的用户点击后浏览器不会有反应（页面会给出
  安装指引）。

## 为什么需要这个页面

桌面应用无法读取用户浏览器的 cookie——这是操作系统与浏览器的安全边界，
任何绕过它的行为都属于凭据窃取。因此「跳到默认浏览器登录」之后把登录态
拿回来，唯一正当的通道是一个**同源页面自愿移交**：站长在自家域名挂一个
握手页，读同源凭据、经用户确认后重定向回应用。

这也是上游的正解未落地前的临时形态。长期方案是为两个上游（sub2api /
new-api）提案的「一次性授权码」契约：站点后台原生生成短时效 code，客户端
用 code 换 token——届时站长零维护、覆盖全部站点，握手页退役。
