<div align="center">

<img src="assets/branding/loongport-icon-master.png" alt="" width="96" height="96">

# LoongPort

### Codex 只花官方的 5%，Claude 只花 20%，国内直连

[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS-lightgrey.svg)](../../releases)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-orange.svg)](https://tauri.app/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

### 🌐 官方网站：**[loongport.dev](https://loongport.dev)**

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
  <img src="assets/screenshots/main-zh.png" alt="LoongPort 主界面：运营商与档位列表，每行显示余额与档位数" width="820">
</div>

## 为什么便宜这么多

两层优惠，Codex 吃到两层，Claude 只吃到一层：

| 因素 | | 谁吃得到 | 为什么 |
|---|---|---|---|
| 档位倍率 | **×0.1** | 只有 Codex | 多数 GPT 档位的计费倍率是 0.1 —— 同样的 token 用量，按官方价的一折计费。Anthropic 分组没有这个折扣，倍率是 1 |
| 汇率口径 | **×1/6.7** | Codex 与 Claude 都有 | 中转站普遍按「1 人民币抵 1 美元」计价，而实际汇率约 1 美元兑 6.7 人民币 |

所以 Codex 是 `0.1 × 1/6.7 ≈ 0.015`，成本约为**官方 API 的 1.5%**；
Claude 是 `1 × 1/6.7 ≈ 0.15`，约为**官方 API 的 15%**。上面写「只花 5%」和「只花 20%」
都留了余量，因为倍率随档位和站点浮动。完整推导与几点说明：
**[loongport.dev/zh/pricing](https://loongport.dev/zh/pricing)**

LoongPort 免费，不经手付款、不从你的余额抽成。你充值给的是中转服务商。

> 注册链接会带上我们的邀请码，我们可能因此从中转站获得返利；其中 `bestapi.store`
> 是我们自己的站，内置的 `LOONGPORT` 优惠码是那里的新用户赠额。都不影响你的价格，
> 域名填谁都行。两张表在 `src-tauri/src/operator/` 的 `aff.rs` 与 `promo.rs` 里。

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
| **Windows** | Windows 10 或更高 |
| **macOS** | macOS 12（Monterey）或更高 |

到 [Releases](../../releases) 页面下载。每个版本都由 GitHub Actions 自动构建，
Windows 与 macOS 各有产物：

| 平台 | 文件 | 说明 |
|---|---|---|
| **Windows** | `LoongPort-v{version}-Windows-Portable.zip` | **免安装版，推荐** —— 解压出一个 `LoongPort.exe` 就能跑，不经过 Windows 安装程序 |
| | `LoongPort-v{version}-Windows.msi` | 安装版（会建开始菜单项） |
| **macOS** | `LoongPort-v{version}-macOS.dmg` | — |

ARM64 的 Windows 机器（如骁龙笔记本）用带 `-arm64` 的那两个，同样是安装版与免安装版各一。

> **Windows 建议直接用免安装版。** 它解压出来只有一个 exe（WebView2 loader 已静态链接，
> 同目录不需要额外 DLL），放到任意位置双击即可。装机的安全软件有时会拦
> Windows 安装程序**备份旧文件**的动作（报 `could not set file security for file
> '...\Config.Msi\xxxxxxx.rbf'  Error: 5`）—— 免安装版没有安装步骤，也就没有可拦的动作。
>
> 前提是系统有 WebView2 运行时，Windows 10 及以上基本自带。

> **macOS 首次打开会被拦一下。** macOS 版未做 Apple 代码签名与公证，所以 Gatekeeper
> 会报「已损坏」—— **不是真的损坏**。终端里执行一次即可，**只需做这一次**：
>
> ```bash
> xattr -dr com.apple.quarantine /Applications/LoongPort.app
> ```
>
> 签名与公证需要 Apple 开发者账号（$99/年），会在后续版本补上 —— 它影响的只是安装
> 这一步，装好之后功能与 Windows 完全一致。

> **已经在用安装版、升级时报「无法设置文件安全性」怎么办。** 少数装了安全软件（如
> 腾讯电脑管家）的机器上，**覆盖安装**旧版本会失败：`could not set file security for
> file '...\Config.Msi\xxxxxxx.rbf'  Error: 5` —— 那是安全软件拦住了安装程序**备份
> 旧文件**的动作，**不是安装包坏了**。先在「设置 → 应用」里卸载旧版本，再双击新安装包：
> 全新安装不需要备份旧文件，就不会被拦。**账号和配置不会丢** —— 它们在用户目录下，
> 卸载不动它们，装回来仍是登录状态。
>
> 首次安装、以及用免安装版的用户都不会遇到这个 —— 它只在覆盖旧版本时出现。

## 怎么用

1. **填域名** —— 你的中转站域名。不确定就留空，用默认的。
2. **登录** —— 弹窗里加载该站真实的登录页，注册或登录都在里面完成。LoongPort 拿到的
   是登录后的凭据，不经手你的密码。
3. **自动备 key** —— 每个可用档位一把。先复用你账号里名字匹配的，找不到才新建，
   所以反复点刷新不会在你账号里堆垃圾 key。
4. **选 CLI 和档位** —— Codex 用 OpenAI 分组、Claude 用 Anthropic 分组，点一下就把
   对应的配置写好（`~/.codex/config.toml` 或 Claude 的 settings）。
5. **继续用** —— Codex 或 Claude Code 直接就能跑。**切 Codex 档位时**会自动退出并
   重开 ChatGPT 桌面版（它只在启动时读配置，不重启新档位不生效）。切 Claude 档位
   不涉及它。

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
  不影响另一个。所以可以一边用 DeepSeek 聊天、一边用中转站的 4K 分组出图。
- **换生图档位不用重启 CLI。** 选择存在 LoongPort 自己的库里、每次生图时现读，CLI 的
  配置文件一个字不动。只有**第一次有生图档位**时需要新开一个终端（那时才往 CLI 配置里
  加那个工具，而 CLI 只在启动时读配置）。
- **没有生图分组的站点会让这一页空着**，你的 CLI 配置一个字不动。而 1K 与 4K 档位
  之间怎么选是花钱的决定，LoongPort 不替你选。

## 怎么升级

**安装版（msi）与 macOS 版会自己检查更新**：启动几秒后在后台问一次，有新版本就在
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

> 一处细节差异，只在**切 Codex 档位**时涉及（那一步要替你重启 ChatGPT 桌面版；
> 切 Claude 档位不碰它）：**macOS 上是「请求退出」**—— 它有进行中的对话时会弹自己的
> 确认框，你可以取消（那次切换随之中止）；**Windows 上是强制结束进程**，不会弹框，
> 所以切换前应用会先告知你一次。

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

# Windows —— 两个参数都不能省。裸 `pnpm tauri build` 会在 MSI 链接那步炸
# （WiX ICE38，本仓用的是 per-user WiX 模板），且 bundle.targets 是 "all"，
# 还会去下载 NSIS 工具链。
pnpm tauri build --target x86_64-pc-windows-msvc --bundles msi
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

见 [CONTRIBUTING.md](CONTRIBUTING.md)。动运营商那条链路前先读
[`LOONGPORT.md`](LOONGPORT.md)，里面记了几处**看着像写错、其实必须那样写**的约束
（`model_provider` 为何必须是 `custom`、为何不能声明 `requires_openai_auth`、
退 ChatGPT 为何按 bundle id）。每条都有测试钉着。

## 许可证

[MIT](LICENSE)。
