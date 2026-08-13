<div align="center">

<img src="assets/branding/loongport-icon-master.png" alt="" width="96" height="96">

# LoongPort

### Codex 只花官方的 5%，Claude 只花 20%，国内直连

[![下载最新版](https://img.shields.io/github/v/release/SailingLoong/LoongPort?label=%E4%B8%8B%E8%BD%BD%E6%9C%80%E6%96%B0%E7%89%88&color=2ea44f&style=for-the-badge)](../../releases/latest)

[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS-lightgrey.svg)](../../releases)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-orange.svg)](https://tauri.app/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

### 🌐 官方网站：**[loongport.dev](https://loongport.dev)**

### 💬 QQ 群：**773696474**

<img src="assets/qq-group.jpeg" alt="LoongPort QQ 群二维码" width="240">

中文 | [English](README_EN.md)

</div>

## 它替你做什么

你想用 Codex CLI 或 Claude Code，但不想按官方 API 的价付钱。照常规做法这意味着：
去中转站注册、找到控制台、手建一把 API Key、把 `base_url` 抄对、翻出配置文件、
把字段写对。换个 CLI 或档位，再来一遍。

LoongPort 把这些压成两步 —— **填一个域名，登录一次。** 它会为你账号能用的每个档位
备好 key、按各 CLI 的形状把配置写好，之后换档位就是点一下。

站点有生图档位的话，还能[**在 CLI 的对话里直接生图**](#在-cli-里生图) ——
而且不用让出你的对话档位。

<div align="center">
  <img src="assets/screenshots/main-zh.png" alt="LoongPort 主界面：中转站与档位列表，每行显示余额与档位数" width="820">
</div>

## 三分钟上手

> **给中转站负责人**：这一节可以直接转给你的用户。他们**不需要**装 cc-switch、
> 不需要在 LoongPort 这边注册任何账号 —— 从下载到能跑起来只有下面四步，
> 期间只跟**你的站**打交道。想让他们默认落到你的站，见[给中转站负责人](#给中转站负责人)。

1. **下载并打开** —— 见[安装](#安装)。第一次打开会弹一个「选择服务站点」的窗。
2. **把中转站的域名粘进去** —— 从浏览器地址栏直接复制也行
   （`https://bestapi.store/usage` 这种，后面的路径会自动去掉）。
3. **注册或登录** —— LoongPort 会打开**这个站自己的**注册页。已有账号的话，
   页面顶部有一条横幅可以一键转去登录。整个过程在这个站的真实页面里完成，
   LoongPort 拿到的是登录后的凭据，**不经手你的密码**。
4. **完事** —— 你账号下能用的每个档位都已备好 key、配置也写好了。之后：
   - **换档位**：点一下「启用」
   - **充值**：点余额旁边的按钮，开这个站自己的充值页
   - **生图**（站点有生图档位时）：见[在 CLI 里生图](#在-cli-里生图)

不用碰配置文件，不用手建 API Key，不用记 `base_url`。

## 给中转站负责人

你可以把 LoongPort 当作**你自己站点的客户端**推给用户 —— 它是通用的 sub2api 客户端，
不绑定任何一家站：

- **省掉你的接入文档。** 用户不再需要照着教程手建 key、抄 `base_url`、找配置文件。
  上面那四步就是全部，而其中三步都发生在你的站上。
- **不抢你的用户关系。** LoongPort 没有账号体系、没有服务端。用户注册的是**你的站**，
  充值走**你的**收款页，凭据只存在他自己电脑上（`~/.loongport/` 的 SQLite）。
- **想让用户默认落到你的站**：LoongPort 会拉一份签名的远端配置，里面可以带「推荐站点」
  列表 —— 它出现在「选择服务站点」那一屏的最上面，用户点一下就连上。
  [开个 issue](../../issues) 说一声即可。
- **有 aff / 优惠码机制的话一并支持**：同一份配置里可以带你的邀请码与注册优惠码，
  用户注册时自动带上。

## 为什么便宜这么多

两层优惠，Codex 吃到两层，Claude 只吃到一层：

| 因素 | | 谁吃得到 | 为什么 |
|---|---|---|---|
| 档位倍率 | **×0.1** | 只有 Codex | 多数 GPT 档位的计费倍率是 0.1 —— 同样的 token 用量，按官方价的一折计费。Anthropic 档位没有这个折扣，倍率是 1 |
| 汇率口径 | **×1/6.7** | Codex 与 Claude 都有 | 中转站普遍按「1 人民币抵 1 美元」计价，而实际汇率约 1 美元兑 6.7 人民币 |

所以 Codex 是 `0.1 × 1/6.7 ≈ 0.015`，成本约为**官方 API 的 1.5%**；
Claude 是 `1 × 1/6.7 ≈ 0.15`，约为**官方 API 的 15%**。上面写「只花 5%」和「只花 20%」
都留了余量，因为倍率随档位和站点浮动。完整推导与几点说明：
**[loongport.dev/zh/pricing](https://loongport.dev/zh/pricing)**

LoongPort 免费，不经手付款、不从你的余额抽成。你充值给的是中转服务商。

> 注册链接会带上我们的邀请码，我们可能因此从中转站获得返利；其中 `bestapi.store`
> 是我们自己的站，内置的 `LOONGPORT` 优惠码是那里的新用户赠额。都不影响你的价格，
> 域名填谁都行。两张表在 `src-tauri/src/relay/` 的 `aff.rs` 与 `promo.rs` 里。

## 不占用官方账号

两边的官方登录都留在原处，想用随时切回去，不必手改任何东西 —— 但两者靠的机制不同：

- **Codex**：ChatGPT 桌面版与命令行 `codex` **共用同一份**凭据文件
  （`~/.codex/auth.json`），所以这里需要显式保留 —— LoongPort 默认开启保留，
  切档位时不写它。
- **Claude**：官方登录在 `~/.claude/.credentials.json`，而切档位写的是同目录的
  `settings.json`，**两个文件本来就分开** —— 不存在被覆盖的问题。

## 安装

| 平台 | 要求 |
|---|---|
| **Windows** | Windows 10 或更高（需 WebView2 运行时，Windows 10 及以上基本自带） |
| **macOS** | macOS 12（Monterey）或更高 |

到 [Releases](../../releases) 页面下载。每个版本都由 GitHub Actions 自动构建：

| 平台 | 文件 | 说明 |
|---|---|---|
| **Windows** | `…-Windows-Setup.exe` | **推荐** —— 安装版，会建开始菜单项并支持应用内自动升级 |
| | `…-Windows-Portable.zip` | 免安装版，解压即用；发现新版后需手动下载 |
| **macOS** | `…-macOS.dmg` | 两种芯片通用 |

ARM64 的 Windows 机器（如骁龙笔记本）用带 `-arm64` 的那两个。

> **macOS 首次打开会被拦一下。** 未做 Apple 签名与公证，Gatekeeper 会报「已损坏」——
> 不是真的损坏，是缺签名。**先拖进「应用程序」、别打开**，然后在终端执行一次：
>
> ```bash
> xattr -dr com.apple.quarantine /Applications/LoongPort.app
> ```
>
> 之后正常打开，只做这一次。这条命令做了什么、为什么安全，都在
> **[loongport.dev/zh/download](https://loongport.dev/zh/download)**。

## 怎么用

1. **填域名** —— 你的中转站域名。从浏览器地址栏整条粘过来也行，不确定就留空用默认的。
2. **登录** —— 弹窗里加载该站真实的登录页，注册或登录都在里面完成。LoongPort 拿到的
   是登录后的凭据，不经手你的密码。
3. **自动备 key** —— 每个可用档位一把。先复用你账号里名字匹配的，找不到才新建，
   所以反复点刷新不会在你账号里堆垃圾 key。
4. **点一下要用的档位** —— Codex 用 OpenAI 那些档位、Claude 用 Anthropic 那些，
   点一下就把对应的配置写好（`~/.codex/config.toml` 或 Claude 的 settings），
   之后 Codex 或 Claude Code 直接就能跑。

> **切 Codex 档位时**会自动退出并重开 ChatGPT 桌面版 —— 它只在启动时读配置，
> 不重启新档位不生效。**macOS 上是「请求退出」**，它有进行中的对话时会弹自己的确认框，
> 你可以取消（那次切换随之中止）；**Windows 上是强制结束进程**，不弹框，所以切换前
> 应用会先告知你一次。切 Claude 档位不涉及它。

凭据与站点信息存在本机 `~/.loongport/` 的 SQLite 里，只作为 Bearer token 发给你选的
那个中转站。LoongPort 没有账号体系、没有服务端，拿不到你的凭据。

## 在 CLI 里生图

只提供生图模型的档位收在**「Codex 生图」这个标签页**里（紧邻 Codex）。在其中一个档位上
点「启用」，之后在 Codex、Claude Code 或 Gemini CLI 的对话里说「生成一张图」即可 ——
生图靠 LoongPort 内置的一个工具（MCP server）完成。

四件值得知道的事：

- **这一页不能单独用，要配合对话档位。** 它只决定「图从哪个档位出」。
- **对话档位不必让出来。** 聊天走 `/v1/responses` 用 Codex 页选的那个档位，生图走
  `/v1/images/generations` 用生图页选的那个 —— 两个「当前项」各自独立，切换任一个都
  不影响另一个。所以可以一边用 DeepSeek 聊天、一边用中转站的 4K 档位出图。
- **换生图档位不用重启 CLI。** 选择存在 LoongPort 自己的库里、每次生图时现读，CLI 的
  配置文件一个字不动。只有**第一次有生图档位**时需要新开一个终端（那时才往 CLI 配置里
  加那个工具，而 CLI 只在启动时读配置）。
- **没有生图分组的站点会让这一页空着**，你的 CLI 配置一个字不动。而 1K 与 4K 档位
  之间怎么选是花钱的决定，LoongPort 不替你选。

## 怎么升级

**Windows Setup 安装版与 macOS 版会自己检查更新**：启动几秒后在后台问一次，有新版本就在
「设置 → 关于」提示，点一下即下载、安装、重启。检查失败不打扰你（离线、连不上 GitHub
都很常见），也可以随时手动点「检查更新」。

> **Windows 免安装版不做原地更新** —— 它没法替换正在运行的自己。它同样会告知有新版本，
> 但要你回 [Releases](../../releases) 下载新的压缩包。这是选免安装版的代价，
> 换来的是没有安装步骤、不会被安全软件拦。

## 支持范围

| | 已支持 | 在做 |
|---|---|---|
| **中转服务** | sub2api | new-api |
| **AI CLI** | codex · claude | gemini · grok |
| **平台** | macOS · Windows | Linux |

站点域名可以自己填，默认预置一个可用的。macOS 与 Windows 功能一致。

> **「AI CLI」那行说的是对话档位。** 生图工具装进的是 codex、claude **与 gemini** 三个
> ——「gemini 在做」指的是它还不能作为对话档位的目标（缺配置写入形状），不是完全没接。

> **「在做」不是愿望清单。** 以 new-api 为例：登录标识已经按它设计成中立的
> `login_identifier`（而不是写死 sub2api 那侧的字段名），`platform_map` 的映射表也建全了
> —— 缺的是它自己那套接口的适配层。**如果你在运营 new-api 站点、或就是它的开发者，
> [开个 issue](../../issues) 能让这件事快很多**：我们需要的是一个能测的站点，
> 以及确认几个端点的形状。同理适用于其它类型的中转站。

## 如果你运营中转站

**可以把它当作自己站点的客户端推给用户** —— 不需要改任何代码，用户填你的域名就能连上。
LoongPort 没有账号体系、没有服务端，不经手流量也不经手钱：用户注册的是你的站、充值付给你，
凭据只存在他自己电脑的 SQLite 里。

它替你做的三件事，每条都可以在源码里核：`normalize_site_origin`
（`relay/api.rs`，域名从地址栏整条粘过来也认）、落你自己注册页的自助注册
（`relay/login.rs`，新站 `/register`、老用户 `/login`，邀请码随 URL 带上）、
注入登录态的自助充值（`relay/purchase.rs`，开 `{你的域名}/purchase`）。

好处、顾虑、技术前提与接入步骤都在
**[loongport.dev/zh/for-relays](https://loongport.dev/zh/for-relays)**。

## 上游项目

**[cc-switch](https://github.com/farion1231/cc-switch)**（作者
[@farion1231](https://github.com/farion1231)，MIT）—— 本项目的基座，从 v3.19.1
fork、已合并上游至 v3.19.2，图标衍生自它，版权声明保留在 [LICENSE](LICENSE) 里。

cc-switch 是通用的多供应商管理器，管所有 CLI 的所有供应商，还有本地代理、MCP、
Skills、Prompts、会话管理。LoongPort 只做「用中转服务省钱跑 AI CLI」这一条链路。
两者数据目录分开（`~/.cc-switch/` 与 `~/.loongport/`），可以同时装、同时开。

**[sub2api](https://github.com/Wei-Shaw/sub2api)**（LGPL-3.0）—— 多数中转站跑的
后端。LoongPort 是它的**纯 HTTP 客户端**，不链接、不包含、也没有复用它的代码，
只依据其公开接口调用。非官方客户端，与其作者无关联 —— 用它遇到的问题请提到本仓。

## 从源码构建

需要 Node.js 22（见 `.node-version`）与 Rust 工具链（版本由 `rust-toolchain.toml`
锁定，rustup 会自动装）。

```bash
git clone https://github.com/SailingLoong/LoongPort.git
cd LoongPort
pnpm install
pnpm dev           # 开发模式，前端热更新
```

打包命令分平台 —— `--bundles app` 出的是 macOS 的 `.app`，在 Windows 上没用：

```bash
# macOS
pnpm tauri build --bundles app

# Windows x64 NSIS Setup（应用内更新也使用同一种安装器）
pnpm tauri build --target x86_64-pc-windows-msvc --bundles nsis
```

macOS 上本机构建出来的应用不带隔离标记，Gatekeeper 不会介入。

<details>
<summary><strong>技术栈与测试</strong></summary>

**前端**：React 18 · TypeScript 5 · Vite 7 · TailwindCSS 3.4 · TanStack Query v5 · shadcn/ui

**后端**：Tauri 2.8 · Rust（edition 2021，版本见 `rust-toolchain.toml`）· serde · tokio · SQLite

```bash
cargo test --manifest-path src-tauri/Cargo.toml   # 后端
pnpm vitest run                                   # 前端
pnpm tsc --noEmit                                 # 类型检查
```

</details>

## 参与贡献

见 [CONTRIBUTING.md](CONTRIBUTING.md)。动中转站那条链路前先读
[`LOONGPORT.md`](LOONGPORT.md)，里面记了几处**看着像写错、其实必须那样写**的约束
（`model_provider` 为何必须是 `custom`、为何不能声明 `requires_openai_auth`、
退 ChatGPT 为何按 bundle id）。每条都有测试钉着。

## 许可证

[MIT](LICENSE)。
