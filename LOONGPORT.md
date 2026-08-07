# LoongPort

> **Codex 只花官方的 5%，Claude 只花 20%，国内直连**
> *Codex at 5% of official cost, Claude at 20%, no extra network setup.*

这两句是**固定副标题**，README / 官网 / GitHub description 第一行都用它，别每处另写一版。
产品名只承担识别（同 Docker / Vercel，不自我描述），"它是什么"由副标题讲 ——
少了这句，`Port` 容易被读成"移植版"，正好是最不想要的解读。

⚠️ **视角是用户的（我用 Codex / Claude，我省钱），不是实现的。** 上一版写的是「一个运营商
账号，直接变成任意 AI CLI 的可用供应商」—— 那句描述的是软件内部干了什么，而用户关心的是
自己得到什么。改这句要同步改三处：`README.md`（中文，GitHub 首页展示的那份）/
`README_EN.md` 的标题下方，
以及官网 `src/i18n/translations.ts` 的 `TAGLINE`。

⚠️ **一律用「只花官方的 N%」这个 level 口径，别改回「省 N%」那个 delta 口径**
（两者等价：花 5% ⟺ 省 95%）。选 level 的三条理由：

1. **与官网 /pricing 的推导同单位** —— 那页算出来的就是「成本约为官方的 1.5%」。
   首屏说 delta，读者从钩子走到证明要自己心算一次 95% ↔ 1.5%。
2. **上限比下限更像买家要的承诺** —— 他想知道「我最多掏多少」。保守表述也随之
   自然：「5% 以内」而实际约 1.5%。
3. **折扣百分比是广告的形状，level 是事实陈述的形状** —— 这产品整个立意是可核性，
   越像促销越触发骗子警报。

也别把两种口径并排写（「只花 5%（省 95%）」）—— 同一件事说两遍。

⚠️ **两个数字别合并成一个**（如笼统取上界写「只花 20%」）：Codex 与 Claude 的成本
来源不同 —— Codex 吃到档位倍率折扣（×0.1）与汇率两层，Claude 只吃到汇率那层
（Anthropic 分组无倍率折扣）。取上界砍掉 Codex 主卖点的一半，取下界则让 Claude
用户按 5% 预期然后落空。

两条**不要**写进对外文案的表述，即使它们更抓人：
- **「无惧封号」** —— 隐含「官方会封号、用我们不会」，是承诺一个 LoongPort 控制不了的
  结果（封号权在运营商与上游）。可自证的说法是「不占用官方账号」：
  `preserveCodexOfficialAuthOnSwitch` 默认开，切档位不动 `auth.json`。
- **「不用科学上网」** —— 会把产品定位成规避网络管制的工具。行业通用表述是
  「国内直连」/「无需额外网络配置」，传达同样的信息。

最初在 [cc-switch](https://github.com/farion1231/cc-switch) v3.19.1 上 fork、之后合并
上游至 v3.19.2 的客户端：
把一个**运营商账号**变成 CLI 可用的多档位供应商 —— 用户填域名、登录、拿到分组，
不必自己建 key、抄 base_url、配 config。

**目标形态是多运营商 × 多 CLI**（sub2api / new-api / … × codex / claude / gemini / …）。
当前进度：

| 维度 | 已实现 | 在做 / 待做 |
|---|---|---|
| 运营商 | sub2api | new-api（登录标识已按它设计成中立的 `login_identifier`） |
| CLI | codex · claude | gemini / grok（`platform_map` 映射表已建全，缺各自的配置写入形状） |
| 平台 | macOS · Windows | Linux |

**Windows 自 2026-08-03 起可用**（MSI 已在维护者机器上打出并验证）。

**ChatGPT 自动退出/重开两个平台都已实现**（Windows 于 2026-08-04 补完）。
但**手段与语义不同，写对外文案时别当成完全等价**：

| | 手段 | 用户能否拒绝 |
|---|---|---|
| macOS | AppleScript `quit`（协作式） | **能** —— 有进行中对话时它会弹自己的确认框，用户点取消即中止切换（`UserDeclined`） |
| Windows | `taskkill /F`（强制） | **不能** —— 所以切换前的告知弹窗是必需的，不是可选的 |

`quit_and_wait` **不返回错误**：退出失败（macOS 权限被拒 / Windows 罕见杀不掉）归到
`NeedsManualRestart`，切换照常进行 + 提示用户自己重启 —— 把「没替用户关掉那个 app」
当失败，会让权限被拒的机器上每次切换都失败，而配置本来是写得进去的。
详见 `chatgpt_app.rs` 顶部那张表与「`WM_CLOSE` 这条路已被实测证伪」那节。

⚠️ **别把「当前只实现了 X」读成「设计上只支持 X」** —— 数据层已按多运营商多平台设计
（四段式 Key 契约含 platform 段、`platform_map` 六个平台全覆盖、命令层签名都吃 `app_id`）。

**配置写入形状**：codex 那份由 `settings_config_for` 里的专属分支写（多一行
`requires_openai_auth`，见那个函数上方的说明）；claude 与其余 CLI 走上游
`build_provider_from_request` 构造 —— sk 落在 `env.ANTHROPIC_AUTH_TOKEN`，
位置由 `api_key_location` 统一定义（`patch_api_key` 与 `extract_api_key` 共用它，
免得两处分叉）。gemini / grok 还缺各自的落法。

上游那份 `README.md` 描述的是 cc-switch 本身（8 个 CLI、本地代理、MCP/Skills 全套），本文件
只讲 LoongPort 自己加的那条链路。

## 它替你做什么

```
①填域名（底纹 790053500.com，可改，留空用默认）
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
pnpm tauri build --bundles app    # 产物 src-tauri/target/release/bundle/macos/LoongPort.app
```

数据落在 `~/.loongport/`（DB 是 `loongport.db`）。**与已装的 cc-switch 完全隔离** —— 那是
`~/.cc-switch/`，两边互不影响。

同时装了 cc-switch 时注意：两边都有 single-instance 插件，但它按 identifier 区分，所以可以
同时开。**同一个 app 开两份则第二份会立刻静默退出**（这是插件的正常行为，不是崩溃）——
debug 版还开着时 release 版起不来，排查时先 `pkill -f LoongPort`。

### dmg 那步会超时，用 hdiutil 手工做

`tauri build`（不带 `--bundles app`）在打 dmg 时会失败：它的 `bundle_dmg.sh` 用 AppleScript
让 Finder 美化窗口布局，那个 Apple event 会超时（`-1712`，与 `chatgpt_app.rs` 处理的是同一
个坑）。app 本身**已经打好了**，只是 dmg 那一步挂掉。

要 dmg 就手工做一个（无花哨布局，功能一样）：

```bash
cd src-tauri/target/release/bundle
# 版本号从 tauri.conf.json 读，别写死 —— 写死的那个会随每次升版过期，
# 而产出一个名字与内容不符的 dmg 是不会报错的。
VER=$(python3 -c "import json;print(json.load(open('../../../tauri.conf.json'))['version'])")
STAGE=$(mktemp -d)/LoongPort && mkdir -p "$STAGE"
cp -R macos/LoongPort.app "$STAGE/" && ln -s /Applications "$STAGE/Applications"
hdiutil create -volname LoongPort -srcfolder "$STAGE" -ov -format UDZO \
  "dmg/LoongPort_${VER}_aarch64.dmg"
rm -rf "$(dirname "$STAGE")"
```

### 产物归档：mac 落 `~/下载`、windows 落 `D:\`，两边同一个约定

产物别留在 `src-tauri/target/release/bundle/`（构建中间目录，下次 build 会清掉），
打完了**复制到固定位置**再试用 / 分发 —— 归档位置就是「装到哪、从哪试」：

| 平台 | 归档目录 | 产物 |
|---|---|---|
| macOS | `~/下载` | `LoongPort.app`（整个 .app 目录） |
| Windows | `D:\`（D 盘根） | `LoongPort_<ver>_x64.msi` / `.exe` |

```bash
# mac：把 .app 复制到 ~/下载
cp -R src-tauri/target/release/bundle/macos/LoongPort.app ~/下载/

# windows（在 Windows 机器 / CI 上）：复制到 D 盘根
# copy /Y src-tauri\target\release\bundle\nsis\LoongPort_*.exe D:\
```

版本号从 `src-tauri/tauri.conf.json` 读，别写死；产物名里带架构后缀
（`aarch64` / `x64`），归档时**别去掉** —— 分发时要能一眼看出是哪个架构的。

## Tauri 命令

**唯一权威是 `lib.rs` 的 `invoke_handler` 注册表**（`generate_handler!` 那一段）——
表里没有的就是不存在，别信任何文档里的清单，包括这一份。

### 中转站（operator）

| 命令 | 干什么 |
|---|---|
| `operator_status` | **只读本地、不发网络** —— 首屏渲染等的就是它 |
| `operator_check_session` | 探活 + 静默续期；凭据真失效时清掉那一行的本地记录并回传 id |
| `operator_probe_site` | 探测域名是不是 sub2api 站，成功即存为站点 |
| `operator_login` | 开登录 WebView，等凭据回来。**按行 id 定位，不回落到「当前站」** |
| `operator_provision` | 拉分组 → 每组备 sk → 按分组自己的平台写成对应 CLI 的 provider |
| `operator_list_operators` | 列已加的运营商与档位（同样不发网络） |
| `operator_list_tier_rates` | 档位倍率 —— 必须发网络，所以从上一条拆出来、首屏之后再填 |
| `operator_switch_tier` | 退 ChatGPT → 切换 → 重开（只有切 Codex 档位才动 ChatGPT） |
| `operator_reset_tier_config` | 重写某档位的 CLI 配置，**复用原 sk 不换新的** |
| `operator_balance` / `operator_purchase` | 查余额 / 打开充值页 |
| `operator_reorder` | 拖拽排序（只在用户明确拖动时调，选档位不重排） |
| `operator_list_sites` / `operator_remove_site` | 列 / 删站点行 |
| `operator_list_sponsors` | 赞助运营商列表（走远端配置，有缓存与内置两层回落） |
| `operator_stats_endpoint_configured` | 匿名统计端点配没配 —— 没配则整条链路与告知弹窗都不启用 |
| `operator_restore_official_login` | 恢复被切走的 Codex 官方登录 |

### 官网直连（vendor）

| 命令 | 干什么 |
|---|---|
| `vendor_list_accounts` | 列已加的官网账号 |
| `vendor_open_login` | 开该平台的登录页 |
| `vendor_provision` | 建/取 key 并写进所有能用它的 CLI。**本地已有明文则零请求** |
| `vendor_balance` | 查余额（币种与类型都与中转站那侧不同） |
| `vendor_remove` / `vendor_reorder` | 删 / 排序 |

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

- **本地代理 / failover**：sub2api 原生支持 codex 的 `/v1/responses`，不需要协议转换。
  `proxy/` 那套留在仓里但不接线。
- **凭据加密**：token 明文存 SQLite。同一个库里已经躺着明文 sk（上游行为），只加密 token
  没有实际收益。要做就两者一起进 keyring。
