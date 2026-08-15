# 远端配置：赞助中转站 + 邀请码

客户端启动后拉这份配置，覆盖编译期内置的邀请码表、并提供首启屏的赞助商列表。
消费它的代码是 `LoongPort/src-tauri/src/relay/remote_config.rs`（那份的模块文档
讲清了为什么要验签、三层回落怎么运作）。

首次上线 2026-08-03。

## v2 provider policy（多站点目录）

v2 provider policy 是独立于 v1 的已签名目录契约，发布端点为：

- `https://config.loongport.dev/v2/directory.json`
- `https://config.loongport.dev/v2/directory.json.sig`

它的明文源是 `public/v2/directory.json`。这份 policy 是「哪些站点、入口、API
基址、模型和授权方式可以被消费」的**唯一权威来源**；签名覆盖 JSON 的原始字节，
因此改动一个空白字符也必须重新签名。当前 v2 policy 的 BestAPI 条目只声明手工 API
key 授权和可用模型，不包含邀请码。

v1 保持原样：已有桌面客户端继续只读取 `v1/config.json` 与其签名，不能因为发布 v2
而删除、改名或迁移 v1 文件。`deploy.sh` 会在部署整个 `public/` 目录前分别验签 v1 与
v2，保证 JSON 和 detached signature 始终一起发布。

可用性探测、延迟、成功率或其他**观测数据**必须单独保存和呈现：它们不是 policy，
也没有修改 policy 或覆盖 policy 决策的权威。变更可消费的站点策略时，只能修改并重新
签名 v2 directory。

更新 v2 policy 的操作顺序：

```bash
$EDITOR public/v2/directory.json
# 授权维护者先在仓库外提供 LOONGPORT_CONFIG_KEY。
./sign-v2.sh
./verify-v2.sh --local-only
./deploy.sh
./verify-v2.sh
```

`verify-v2.sh --local-only` 验证本地待发布 JSON 的原始字节签名和 schema；不带参数时，
它还会下载线上 JSON 与 detached signature，验签并要求线上 JSON 与本地逐字节一致。
授权维护者在仓库外通过运行时环境变量 `LOONGPORT_CONFIG_KEY` 提供 Ed25519 私钥。
私钥绝不提交、复制到文档或写入日志；密钥配置与恢复请查本机 `--help` 或组织私有 runbook。

## 日常：加一家赞助商 / 改一个邀请码

```bash
cd remote-config
$EDITOR public/v1/config.json     # 1. 改内容
./sign.sh                         # 2. 重新签名（**忘了这步 = 客户端全部拒绝**）
./deploy.sh                       # 3. 部署
./verify.sh                       # 4. 验线上（等 ~30 秒再跑，CDN 有 300 秒缓存）
```

四步都要跑。`sign.sh` 与 `verify.sh` 各自会自验并在出错时明确报出来。

`sign.sh` 和 `sign-v2.sh` 都要求授权维护者先在仓库外提供运行时环境变量
`LOONGPORT_CONFIG_KEY`。私钥绝不提交或记录到日志；供应和恢复流程请查本机 `--help`
或组织私有 runbook。部署同样要求运行时 `CLOUDFLARE_API_TOKEN`；部署凭据必须留在
仓库外，并按组织私有 runbook 提供。

CI 层还有一道一致性闸：`cargo test` 里的
`checked_in_config_passes_the_clients_own_gate`（`remote_config.rs`）用客户端
生产公钥验仓内 `.sig`、并用客户端 DTO 严格解析仓内 `config.json` —— 忘了重签、
或写出客户端解不出的形状，测试直接红。

**不需要发版** —— 这套机制存在的全部意义就是改配置不用发新版本客户端。

## 配置源位置

配置源文件就在本仓库，用户可以直接审阅明文内容；客户端运行时从 Cloudflare Pages
拉取同一份已签名配置。推荐站点与邀请码的变更必须修改明文后重新签名、部署。

`aff_codes` 与 `sponsors` 独立维护：前者负责登录时自动带上注册邀请码，后者负责
添加中转站弹窗的推荐列表。两者都保留客户端编译期回退，确保首次离线启动仍可使用邀请码。

`relay_directory` 只维护广场的兼容策略：`blocked_hosts`、LoongPort host 到 VeriDrop host
的别名、注册/登录入口、购买入口和展示名。排名、评分、样本与日期不写进 Git，始终以
VeriDrop 公开榜单为唯一来源。修改后同样必须重新签名。

每个 `relay_directory.sites` 条目里的 URL 各有一个职责：

- `entry_url` 是用户从广场进入站点注册/登录流程的地址，可以带站点要求的路径。
- `purchase_url` 是登录后进入充值/购买页面的地址。客户端只接受已签名配置中的 HTTPS
  同源地址；字段缺失或为空表示不提供购买入口，绝不猜测 `/purchase`、`/wallet` 等路径。

## ⚠️ 收录一家之前先测它探不探得通

客户端靠 `GET /api/v1/settings/public` 里的 `version` 字段认「这是不是 sub2api 站」
（`relay/api.rs::probe_site`）。**有些站被 Cloudflare 的机器人防护挡在门外**，
那时客户端拿到一个 HTML 挑战页、报「看起来不是 sub2api 站点」——
放进推荐列表就是给用户一个点了必然失败的按钮。

```bash
curl -sS -m 12 -o /dev/null -w "%{http_code}\n" https://<域名>/api/v1/settings/public
```

**200 才收录**（推荐列表的判据不变）。403 说明被防护拦了 —— 那不是「不是 sub2api」，
也**不是加头加 cookie 能绕的**：客户端已经带了浏览器 UA（`api.rs` 里的
`WEBVIEW_USER_AGENT`），实测再加 `Accept` / `Accept-Language` / `Referer` / `Origin`
仍是 403 ⇒ 拦在 **TLS 指纹层**（JA3），非浏览器 HTTP 栈（curl / reqwest）一律被拦。

2026-08-10 实测：`wawapii.com` / `hapiopen.cc` / `999555999.com` 都 200，
**`api.aijws.com`（贾维斯）403** ⇒ 已从 `sponsors` 移除。
但它的 **aff 码留在 `aff_codes` 里** —— 用户自己手动加那个站时返利仍该算我们的。

⚠️ **403 不拦用户手动加站**：这类站的登录在真实浏览器（登录窗）里完成，天然过防护。
2026-08-13 起，reqwest 被 403 拦下的请求会走「登录窗在页面上下文代拉」的浏览器桥
（`relay/browser_bridge.rs` + `Client::send` 的 fallback）：登录、登录后自动备 key
（分组 / 密钥 / 建 key）都在登录窗还开着时借同一扇窗重放请求，
`api.aijws.com` 这类站可以正常登录、显示账号名、自动备好密钥。

**仍受限**：登录窗已关后的 reqwest 操作（如重启 app 后各行的**余额展示**）——
窗口不在就没有浏览器可借，会回到原来的 403 报错（余额失败是静默的，不打断主流程）。
用户重新登录后，该行的备 key 与后续操作又能走通。

## 事实表

| 项 | 值 |
|---|---|
| 端点 | `https://config.loongport.dev/v1/config.json`（签名同名加 `.sig`） |
| 托管 | Cloudflare Pages 项目 `loongport-config`（与官网 `loongport-website` **分开**） |
| DNS | `config` CNAME → `loongport-config.pages.dev`，proxied |
| 公钥 | `3e199ad0082b525fdf8edef5f7161270675e107fd81d31dbce1b71d83936a131` |
| 缓存 | `max-age=300`（见 `public/_headers`） |

## ⚠️ 两件不可逆的事

**端点 URL** 与**那把公钥**都烧在已发布的客户端二进制里。改任何一个，老版本客户端
就永久收不到更新（静默回落到内置表，**不报任何错**）。

⇒ **私钥丢了或泄露是真麻烦，不是「重新生成一把就行」** —— 换公钥要发新版，
而没升级的用户永远停在内置那份。密钥处置必须遵循组织私有 runbook。

（`v1` 那段路径是给将来 schema 破坏性变更留的：那时新客户端打 `/v2/`，
`/v1/` 要一直留着喂旧客户端。）

## 格式

```json
{
  "issued_at": "2026-08-03T00:00:00Z",
  "sponsors": [
    { "site_origin": "https://example.com", "display_name": "示例", "tagline": "一句话介绍" }
  ],
  "aff_codes": { "example.com": "CODE12345678" }
}
```

字段名是**签名覆盖的契约**（`RemoteConfig` / `Sponsor` 的 serde 默认 snake_case）——
改名意味着旧客户端解不出新配置。四条写错了不报错、只静默失效的规则：

- **`aff_codes` 的 key 必须是归一后的 host**：小写、**去 `www.`**、**不带端口**、不带 scheme。
  写 `https://www.example.com` 永远命中不到（归一逻辑见 `aff.rs::lookup_host`）。
- **码按最终大写形态写**。服务端会 `ToUpper`，客户端只 trim 不校验格式。
- **空字符串 = 明确撤销这个站的码**，不回落到内置表。这是撤码的唯一手段。
- **`sponsors` 的顺序就是 UI 展示顺序**，客户端不排序；`display_name` 直接显示，不翻译。

`issued_at` 客户端目前**忽略**（未知字段跳过）。它是为将来防回滚攻击攒的历史 ——
CDN 或攻击者可以重放一份**旧的、签名仍然有效**的配置，把已撤销的码复活。
从第一份就带上，将来加校验时不需要过渡期。

## 脚本

| 脚本 | 干什么 | 什么时候跑 |
|---|---|---|
| `sign.sh` | 签名，然后**用代码里那把公钥**验一遍 | 每次改完 `config.json` |
| `sign-v2.sh` | 签名 v2 directory，再用 production public key 自验 | 每次改完 `directory.json` |
| `deploy.sh` | 先本地验签，通过才部署到 Pages | 签完 |
| `verify.sh` | 拉**线上**那两个文件验签，并比对与本地是否一致 | 部署后 |
| `verify-v2.sh` | 验 v2 policy；`--local-only` 不访问网络 | 签名后、部署后 |
| `lib.sh` | 三者共用的函数（从 `.rs` 取常量、hex→DER、验签），**不单独执行** | — |

## 公开观测数据

`/v2/observations.json` 是一个非权威的公开观测源：Function 仅抓取 VeriDrop 的公开总榜
`https://veridrop.org/leaderboard/`，并返回经过归一化的站点主机名、排名、分数、样本数、观测日期、
VeriDrop 报告链接和问题文本。它的响应缓存策略为
`public, max-age=300, stale-while-revalidate=900`。

部署脚本通过 `--cwd remote-config` 运行 `wrangler pages deploy public`，让 Pages 从同一项目根目录
发现 `functions/`。可用下面的命令本地验证路由：

```bash
npx wrangler pages dev public --cwd remote-config --port 8788
curl -fsS http://127.0.0.1:8788/v2/observations.json | jq .
```

所有需要签名或验签的可执行脚本都用**从 `remote_config.rs` grep 出来的**公钥，不手抄 ——
手抄多一个能写错的地方，而写错的症状是「验签永远失败」，与「服务器挂了」一模一样。

⚠️ **必须用 Homebrew 的 OpenSSL，macOS 自带的 LibreSSL 做不了 Ed25519**
（实测 LibreSSL 3.3.6 连私钥都载不进来，且 `pkeyutl` 没有 `-rawin`）。
这些可执行脚本开头都有一道预检会明确报出来 —— 没有它的话，LibreSSL 下的失败会被
`verify.sh` 报成「改完 JSON 忘了重签？」，**把一份正确的配置误诊成签名出错**。
这台机器能跑只因为 Homebrew 的 OpenSSL 在 PATH 里靠前；另外两台机器上要先：

```bash
brew install openssl@3
export PATH="$(brew --prefix openssl@3)/bin:$PATH"
```

各自守的是不同的东西，别省任何一步：

- **`sign.sh` 的自验**用代码里那把公钥（而不是从私钥现导出的那把）。后者是套套逻辑、
  永远成立；前者才抓得出「私钥换了/指错了，而代码里的公钥没同步」。
- **`deploy.sh` 的前置验签**挡的是「陈旧或不相干的 `.sig`」。曾经这里只判「签名 64 字节」
  和「签名比配置新」，两条都是**代理指标** —— 一个 touch 过的旧签名全能骗过去，
  然后部署上线被客户端整份丢弃，而线上看起来一切正常。
- **`verify.sh`** 覆盖的是单测覆盖不到的那件事：**线上那份与客户端烧着的公钥是否配套**。
  单测用现场生成的密钥对验「机制」对不对，验不了「这次发布对不对」。
  它还比对线上与本地是否一致 —— 只验签通过不够，线上可能是**上一次**发布的
  （旧但签名有效）。

代码侧另有一条等价的闸：`cargo test --lib live_remote_config -- --ignored`
走客户端生产路径打线上端点，两者互为备份。

## 常见故障

| 症状 | 原因 | 修法 |
|---|---|---|
| `verify.sh` 报验签失败 | 改了 JSON 忘跑 `sign.sh`，或只发了一个文件 | 重跑 `sign.sh` + `deploy.sh` |
| `verify.sh` 报「与本地不一致」 | 还没部署，或 CDN 缓存未过期 | 等 5 分钟再试 |
| 客户端拉不到但线上验签通过 | 正常 —— 拉不到不是错误，它会用缓存/内置 | 看客户端日志 `远端配置` |

⚠️ **只发 `config.json` 不发 `.sig` = 客户端全部拒绝。那是正确行为，不是 bug**
（否则攻击者删掉 `.sig` 即可绕过验签）。`deploy.sh` 传整个 `public/`，不会漏。

## 为什么要签名（不是过度设计）

绝大多数远端配置不签名，直接 HTTPS 拉 JSON 就够了。这份不同的地方在于**被篡改的后果**：

- 改 `aff_codes` → 返利收益转给攻击者
- 改 `sponsors` 的 `site_origin` → **用户被引到钓鱼站，并在那里输入真实账号密码**

HTTPS 只保证"传输中没被改"，不保证"服务器上那份就是我们写的"。绕过 TLS 有三条现成的路：
Cloudflare 账号/Token 泄露、域名过期被抢注、CDN 投毒。签名把信任从"服务器和账号"
移到"一把只在本机的私钥"—— 攻击者拿到 Cloudflare 完全控制权也只能让客户端**拉不到**，
不能让它拉到**假的**。

同类做法：apt / Homebrew / Sparkle / Tauri 自己的 updater 都签。
