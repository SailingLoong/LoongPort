# LoongPort V2

在 [cc-switch](https://github.com/farion1231/cc-switch) v3.19.1 上 fork 的极简客户端：
**只服务 codex、只对接 sub2api、只做 macOS**。

上游那份 `README.md` 描述的是 cc-switch 本身（8 个 CLI、本地代理、MCP/Skills 全套），本文件
只讲 LoongPort 自己加的那条链路。

## 它替你做什么

```
①填域名（底纹 bestapi.store，可改，留空用默认）
   └→ ②弹窗加载该站登录页，注册或登录
        └→ ③自动为每个可用分组备好 sk（用户无感）
             └→ ④点一个分组就切过去
                  └→ ⑤切换前退出 ChatGPT，切完自动开回来
```

第 ③ 步是「认领优先」：先在你账号里找名字匹配的 Key 复用，找不到才建新的。所以重复点
「刷新」不会给你的账号堆垃圾 Key。

## 跑起来

```bash
# 依赖（首次）
pnpm install

# 开发模式（前端热更新）
pnpm tauri dev

# 出一个能双击的 app
pnpm tauri build          # 产物在 src-tauri/target/release/bundle/
```

数据落在 `~/.loongport/`（DB 是 `loongport.db`）。**与已装的 cc-switch 完全隔离** —— 那是
`~/.cc-switch/`，两边互不影响。

## 六个 Tauri 命令

| 命令 | 干什么 |
|---|---|
| `operator_status` | 只读本地，决定 UI 显示哪一屏 |
| `operator_check_session` | 探活 + 静默续期；凭据真失效时清掉本地记录 |
| `operator_probe_site` | 探测域名是不是 sub2api 站，成功即存为当前站点 |
| `operator_login` | 开登录 WebView，等凭据回来 |
| `operator_provision` | 拉分组 → 每组备 sk → 写成 codex provider |
| `operator_list_tiers` | 列已备好的档位 |
| `operator_switch_tier` | 退 ChatGPT → 切换 → 重开 |
| `operator_logout` | 清凭据（保留站点与 device-id） |

## 改代码前必读的几条

这些都是实测踩出来的，改错了不会有编译错误、只会静默走歪。每条在源码里都有对应的测试
钉住。

### 1. codex 的 `config.toml` 不能声明 `requires_openai_auth`

`codex doctor` 三组对照：

| config.toml | reachability mode | 实际打到哪 |
|---|---|---|
| `requires_openai_auth = true` + bearer token | ChatGPT auth | chatgpt.com（403，1 fail） |
| 无 `requires_openai_auth` + bearer token | provider auth | 运营商 `/v1`（200，0 fail） |
| `requires_openai_auth = true` + auth.json 有 key | API key auth | 运营商 `/v1`（200，0 fail） |

LoongPort 走第二行（sk 只进 config.toml，不碰 auth.json）。上游预设与 sub2api 面板给的模板
都写 `requires_openai_auth = true`，因为它们走第三行 —— **照抄会落到唯一跑不通的第一行**。

### 2. `model_provider` 必须是 `custom`

它是会话历史的桶标识：所有 provider 都写 `custom`，切换分组后历史才在同一个列表里
（「聊天记录合并」靠的是这个，不是某个设置开关）。

sub2api 面板模板写的是 `model_provider = "OpenAI"` —— `openai` 在 codex 的保留 id 列表里且
比对大小写不敏感，照抄会让 bearer token 落到顶层而非 provider 作用域，且把桶变成 `OpenAI`。

### 3. `~/.codex/auth.json` 是 ChatGPT 桌面版的登录凭据

那个 app（bundle id **`com.openai.codex`**，显示名才叫 ChatGPT）自带一份 codex 核心二进制，
与命令行 codex 共用同一个 `~/.codex`。所以 `preserveCodexOfficialAuthOnSwitch` 在 LoongPort
**默认开**（上游默认关）—— 关掉意味着每次切分组都把你的 ChatGPT 登录清掉。

### 4. 退 ChatGPT 一律按 bundle id，且判据是轮询

- 它内部那份 codex 二进制与命令行 codex **同名**，`pkill -9 -x codex` 实测会把它一起杀掉。
- `quit` 是异步的，且它在有进行中对话时会弹阻塞式确认框；用户点 Cancel 时 `osascript`
  **仍可能返回 rc=0**。所以「已退出」的唯一判据是轮询 `is running` 变 false。
- AppleScript 必须包 `with timeout`：AppleEvent 默认超时是 **120 秒**，不包就是在确认框弹出时
  把 Tauri command 卡两分钟。

### 5. sub2api 的响应信封 `code` 是整数，成功是 `0`

`message` 才是 `"success"`。把它当成 code 会让每一次 API 调用都失败在反序列化上。

而**鉴权中间件（401/403）用的是另一套信封**，那边 `code` 是字符串错误码 —— 两套不能混。

### 6. `api_base_url` 可能是空串

bestapi.store 实测就是。补 `/v1` 的责任在客户端，别指望后台配对。

## 测试

```bash
cargo test --manifest-path src-tauri/Cargo.toml          # 全量（不含联网那条）
cargo test --manifest-path src-tauri/Cargo.toml --lib probe_live_site -- --ignored --nocapture
```

最后那条打真实站点，验的是**契约漂移** —— 上游改字段名时纯函数单测全绿而它会红。它默认
`#[ignore]`（CI 不该依赖外网可达）。

`tests/loongport_codex_live.rs` 三条守整条落盘链路，其中一条断言切换分组后 `auth.json`
**逐字节未变**。

## 已知不做的

- **Windows / Linux**：退出/重开 ChatGPT 只有 macOS 实现，其它平台返回明确错误让 UI 降级成
  「请手动重启」。要加 Windows 时改动局限在 `operator/chatgpt_app.rs` 与
  `operator/login.rs`（后者的凭据回传走自定义 scheme 导航拦截，WebView2 上的行为需要真机
  验一次；回退方案是 V1 那套 `document.title` 分片协议）。
- **本地代理 / failover**：sub2api 原生支持 codex 的 `/v1/responses`，不需要协议转换。
  `proxy/` 那套留在仓里但不接线。
- **自动更新**：`plugins.updater` 整块删了（上游端点会把用户升级成 cc-switch）。有自己的
  发布渠道之后要同时配 endpoints + pubkey + 加回 `lib.rs` 里那段注册，缺一个都不行。
- **凭据加密**：token 明文存 SQLite。同一个库里已经躺着明文 sk（上游行为），只加密 token
  没有实际收益。要做就两者一起进 keyring。
