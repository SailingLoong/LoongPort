<div align="center">

<img src="assets/branding/loongport-icon-master.png" alt="" width="96" height="96">

# LoongPort

### Codex 只花官方的 5%，Claude 只花 20%，国内直连

[![下载最新版](https://img.shields.io/github/v/release/SailingLoong/LoongPort?label=%E4%B8%8B%E8%BD%BD%E6%9C%80%E6%96%B0%E7%89%88&color=2ea44f&style=for-the-badge)](../../releases/latest)

[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS-lightgrey.svg)](../../releases)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-orange.svg)](https://tauri.app/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

### 🌐 官方网站：**[loongport.dev](https://loongport.dev)**

### 📖 图文教程：**[从下载到接上中转站，含从 cc-switch 迁移](https://github.com/SailingLoong/cc-switch-relay-tutorial)**

### 💬 QQ 群：**773696474**

<img src="assets/qq-group.jpeg" alt="LoongPort QQ 群二维码" width="240">

中文 | [English](README_EN.md)

</div>

## 它替你做什么

以低于官方 API 的成本运行 Codex CLI 与 Claude Code，常规做法是一套繁琐流程：在中转站注册账号、定位控制台、手动创建 API Key、正确复制 `base_url`、找到并准确修改配置文件；更换 CLI 或档位后，整套流程还需重来一遍。

LoongPort 将其压缩为两步 —— **填入一个域名，登录一次。** 应用会为账号可用的每个档位备好密钥，按各 CLI 的配置形状完成写入；此后更换档位只需一次点击。

除中转站档位外，LoongPort 还内置[官网直连档位](#官网直连官方-api)（DeepSeek、智谱 BigModel、opencode），两者并列参与[省心模式](#省心模式把挑档位交给系统)的自动调度。站点提供生图档位时，还可以[**在 CLI 的对话里直接生图**](#在-cli-里生图)，且无需让出对话档位。

<div align="center">
  <img src="assets/screenshots/main-zh.png" alt="LoongPort 主界面：中转站与档位列表，每行显示余额与档位数" width="820">
</div>

## 三分钟上手

> **给中转站负责人**：本节可直接转发给你的用户。他们**无需**安装 cc-switch，也**无需**在 LoongPort 注册任何账号 —— 从下载到可用只有以下四步，且全程只与**你的站点**交互。若希望用户默认接入你的站点，见[给中转站负责人](#给中转站负责人)。

1. **下载并打开** —— 见[安装](#安装)。首次启动会弹出「选择服务站点」窗口。
2. **粘贴中转站域名** —— 直接从浏览器地址栏整条复制亦可（如 `https://bestapi.store/usage`，多余路径会自动去除）。
3. **注册或登录** —— LoongPort 会打开**该站点自己的**注册页。已有账号时，页面顶部的横幅可一键转至登录页。整个过程在该站点的真实页面中完成，LoongPort 仅获取登录后的凭据，**不接触你的密码**。
4. **完成接入** —— 账号下可用的每个档位均已备好密钥并完成配置。此后：
   - **更换档位**：点击「启用」即可
   - **充值**：点击余额旁的按钮，打开该站点自己的充值页
   - **生图**（站点提供生图档位时）：见[在 CLI 里生图](#在-cli-里生图)

无需修改配置文件，无需手动创建 API Key，无需记忆 `base_url`。

## 给中转站负责人

你可以将 LoongPort 作为**你自己站点的客户端**推荐给用户 —— 它是通用的 sub2api / new-api 客户端，不绑定任何一家站点：

- **接入文档可以省去。** 用户无需按教程手动创建密钥、复制 `base_url`、查找配置文件。上述四步即为全部，其中三步发生在你的站点上。
- **不介入你与用户的关系。** LoongPort 没有账号体系、没有服务端。用户注册的是**你的站点**，充值走**你的**收款页，凭据仅存储在其本机（`~/.loongport/` 的 SQLite）。
- **希望用户默认接入你的站点**：LoongPort 会拉取一份带签名的远端配置，其中可包含「推荐站点」列表 —— 该列表显示在「选择服务站点」界面顶部，用户点击即可接入。[提交 issue](../../issues) 告知即可。
- **支持 aff / 优惠码机制**：同一份配置可携带你的邀请码与注册优惠码，在用户注册时自动附带（该机制目前面向 sub2api 站点）。

## 为什么便宜这么多

两层优惠叠加，Codex 两者均可享受，Claude 仅享受其一：

| 因素 | | 适用范围 | 说明 |
|---|---|---|---|
| 档位倍率 | **×0.1** | 仅 Codex | 多数 GPT 档位的计费倍率为 0.1 —— 同样的 token 用量，按官方价的一折计费。Anthropic 档位没有该折扣，倍率为 1 |
| 汇率口径 | **×1/6.7** | Codex 与 Claude 均适用 | 中转站普遍按「1 人民币抵 1 美元」计价，而实际汇率约为 1 美元兑 6.7 人民币 |

因此 Codex 为 `0.1 × 1/6.7 ≈ 0.015`，成本约为**官方 API 的 1.5%**；Claude 为 `1 × 1/6.7 ≈ 0.15`，约为**官方 API 的 15%**。上文「只花 5%」与「只花 20%」均为留有余量的保守表述，因为倍率随档位和站点浮动。完整推导与相关说明见 **[loongport.dev/zh/pricing](https://loongport.dev/zh/pricing)**。

LoongPort 本身免费，不经手付款、不从你的余额抽成。你充值的对象是中转服务商。

> 注册链接会附带我们的邀请码，我们可能因此从中转站获得返利；其中 `bestapi.store` 是我们自运营的站点，内置的 `LOONGPORT` 优惠码是该站的新用户赠额。以上均不影响你的价格，域名可任意填写。两张表位于 `src-tauri/src/relay/` 的 `aff.rs` 与 `promo.rs`。

## 不占用官方账号

两侧的官方登录均保留原状，可随时切回，无需手动修改任何内容 —— 但两者依赖的机制不同：

- **Codex**：ChatGPT 桌面版与命令行 `codex` **共用同一份**凭据文件（`~/.codex/auth.json`），因此需要显式保留 —— LoongPort 默认开启保留，切换档位时不写入该文件。
- **Claude**：官方登录位于 `~/.claude/.credentials.json`，而档位切换写入的是同目录的 `settings.json`，**两个文件本就分离** —— 不存在被覆盖的问题。

## 安装

| 平台 | 要求 |
|---|---|
| **Windows** | Windows 10 或更高（需 WebView2 运行时，Windows 10 及以上基本自带） |
| **macOS** | macOS 12（Monterey）或更高 |

前往 [Releases](../../releases) 页面下载。每个版本均由 GitHub Actions 自动构建：

| 平台 | 文件 | 说明 |
|---|---|---|
| **Windows** | `…-Windows-Setup.exe` | **推荐** —— 安装版，会创建开始菜单项并支持应用内自动升级 |
| | `…-Windows-Portable.zip` | 免安装版，解压即用；发现新版本后需手动下载 |
| **macOS** | `…-macOS.dmg` | 两种芯片通用 |

ARM64 架构的 Windows 设备（如骁龙笔记本）请使用带 `-arm64` 后缀的两个文件。

> **macOS 首次打开会被系统拦截。** 由于未做 Apple 签名与公证，Gatekeeper 会报「已损坏」—— 并非真的损坏，而是缺少签名。**请先拖入「应用程序」、暂不打开**，然后在终端执行一次：
>
> ```bash
> xattr -dr com.apple.quarantine /Applications/LoongPort.app
> ```
>
> 之后即可正常打开，此操作仅需一次。该命令的作用与安全性说明见 **[loongport.dev/zh/download](https://loongport.dev/zh/download)**。

## 怎么用

1. **填入域名** —— 你的中转站域名。直接从浏览器地址栏整条粘贴亦可；不确定时留空使用默认站点。
2. **登录** —— 弹窗内加载该站点真实的登录页，注册或登录均在此完成。LoongPort 仅获取登录后的凭据，不接触你的密码。
3. **自动备好密钥** —— 每个可用档位一把。优先复用账号中名称匹配的现有密钥，仅在找不到时新建，因此反复刷新不会在账号中留下冗余密钥。
4. **点击要使用的档位** —— Codex 使用 OpenAI 系档位、Claude 使用 Anthropic 系档位，点击即可写入对应配置（`~/.codex/config.toml` 或 Claude 的 settings），随后 Codex 或 Claude Code 即可直接运行。

> **切换 Codex 档位时**会自动退出并重新打开 ChatGPT 桌面版 —— 它仅在启动时读取配置，不重启则新档位不生效。**macOS 上为「请求退出」**：存在进行中的对话时它会弹出自己的确认框，可选择取消（该次切换随之中止）；**Windows 上为强制结束进程**，不弹框，因此切换前应用会先行告知一次。切换 Claude 档位不涉及该流程。

系统托盘支持在不打开主窗口的情况下快捷切换各应用的档位，并可直接调整省心模式的策略与模型偏好。

凭据与站点信息存储在本机 `~/.loongport/` 的 SQLite 中，仅作为 Bearer token 发送给你选定的中转站。LoongPort 没有账号体系、没有服务端，无法获取你的凭据。

## 省心模式：把挑档位交给系统

接入站点后，「当前使用哪个档位」仍是一项日常决策：不同档位的倍率、单价与实时状况各不相同。省心模式将这一决策交给系统：

- **自动挑档** —— 按「价格最低（倍率 × 模型单价）」或「响应最快（近期平均首字耗时）」两种策略自动选择托管档位；策略与模型偏好按应用分别配置。
- **会话保持** —— 同一会话内保持当前档位不切换，避免丢失提示词缓存。
- **故障转移** —— 当前档位持续失败时按策略顺序自动切换至下一家，恢复后自动切回。
- **档位看板** —— 开启后主页变为档位看板，每个档位的倍率、预估单价、余额、首字耗时与当前命中一目了然，并整合[模型验真](#模型验真)的异常标记。
- **手动优先级** —— 不希望交给系统时，可切换为手动排序，拖拽调整档位优先级，松手即生效；会话保持与故障转移照常保留。
- **随时开关** —— 顶栏常驻开关可随时开启或关闭当前应用。开启时会一并开启本地路由并接管该 CLI 的配置，关闭时自动恢复原有配置。

省心模式位于「设置 → 省心模式」。不开启时一切保持原状：手动切换与故障转移队列照常可用，底层路由配置收在设置「高级」页。

## 官网直连（官方 API）

除中转站外，LoongPort 内置三家厂商的官网直连档位：**DeepSeek 开放平台**、**智谱 BigModel**（GLM 系列）与 **opencode 官网账号**（同一账号展开 Zen 按量计费与 Go 订阅两个档位）。在「官方 API」页登录一次，应用即自动完成密钥获取与各平台配置。

官网直连档位与中转站档位在系统中地位相同：均按官方价目参与省心模式的比价、排序与故障转移。

## 模型验真

档位实际提供的模型是否与标称一致，直接影响计费与输出质量。LoongPort 提供两层核验：

- **主动验证** —— 在档位上发起「模型验证」，对所选模型逐项探测并留存记录，可随时复查。
- **被动观察** —— 本地路由运行期间，托管档位的响应会被被动比对模型指纹，不发起任何额外请求；发现「疑似换芯」即在档位看板标记异常。被动观察只报告异常、不为正常档位作背书，正常档位零打扰。

## 使用统计与扣费对账

经本地路由（含省心模式）的流量会被完整记录，全部仅存本机：请求数、成功率、成本、输入/输出与缓存 token、按日趋势，以及按供应商与模型的分组统计。

中转站账号行提供「扣费对账」：将本地估算成本与站点余额快照的实际扣减按时间窗对照并给出比值，实际扣减显著高于估算的窗口会被标出，便于发现计费异常。该功能在相应应用开启省心模式、产生对账数据后可用。

## 在 CLI 里生图

仅提供生图模型的档位收录在**「Codex 生图」标签页**（紧邻 Codex）。在其中一个档位上点击「启用」后，即可在 Codex、Claude Code 或 Gemini CLI 的对话中直接生成图片 —— 生图由 LoongPort 内置的工具（MCP server）完成。

四点说明：

- **该标签页不能单独使用，需配合对话档位。** 它仅决定「图片从哪个档位产出」。
- **无需让出对话档位。** 对话走 `/v1/responses`、使用 Codex 页选定的档位；生图走 `/v1/images/generations`、使用生图页选定的档位 —— 两个「当前项」相互独立，切换任一均不影响另一个。因此可以一边用 DeepSeek 对话、一边用站点的 4K 档位出图。
- **更换生图档位无需重启 CLI。** 该选择存储在 LoongPort 自己的数据库中、每次生图时读取，CLI 的配置文件不作改动。仅**首次添加生图档位**时需要新开一个终端（此时才会向 CLI 配置写入该工具，而 CLI 仅在启动时读取配置）。
- **未提供生图分组的站点中该页为空**，CLI 配置不作任何改动。1K 与 4K 档位之间的选择属于消费决策，LoongPort 不代为选择。

## 怎么升级

**Windows Setup 安装版与 macOS 版会自动检查更新**：启动数秒后在后台查询一次，有新版本时在「设置 → 关于」中提示，点击即可下载、安装并重启。检查失败不会造成打扰（离线、无法连接 GitHub 的情况均属常见），也可随时手动点选「检查更新」。

> **Windows 免安装版不支持原地更新** —— 它无法替换正在运行的自身。该版本同样会提示新版本，但需回到 [Releases](../../releases) 下载新的压缩包。这是免安装版的取舍：换得的是没有安装步骤、不会被安全软件拦截。

## DeepSeek Harness（dsh）

LoongPort 提供 [`loongport` npm Cordis bundle](https://www.npmjs.com/package/loongport)，可在 [DeepSeek Harness（dsh）](https://github.com/deepseek-ai/deepseek-harness)中使用已验证服务商。

```bash
dsh plugin --profile <profile> add loongport
```

安装后，在 **Settings → LoongPort** 选择服务商；需要账号时打开其自己的注册或登录页面，手动生成并粘贴 API Key，再选择 `deepseek-v4-flash` 或 `deepseek-v4-pro` 保存。默认服务商为 DeepSeek 官方 API，BestAPI 为已验证的中转站选项。完整安全边界、签名目录与 VeriDrop 的职责说明见 **[loongport.dev/zh/dsh](https://loongport.dev/zh/dsh)**。

浏览器授权不做自动化；高级自定义 endpoint 的 CLI 用法见官网。

## 支持范围

| | 已支持 | 在做 |
|---|---|---|
| **中转服务** | sub2api · new-api | — |
| **AI CLI** | codex · claude | gemini · grok |
| **平台** | macOS · Windows | Linux |

站点域名可自行填写，默认预置一个可用站点。macOS 与 Windows 功能一致。

> **「AI CLI」一行指对话档位。** 生图工具可安装至 codex、claude **与 gemini** 三个 CLI ——「gemini 在做」指它尚不能作为对话档位的目标（配置写入形状尚未完成），并非完全未接入。

> **「在做」不是愿望清单。** gemini 与 grok 的档位映射表已经建全，缺的是各自的配置写入适配。如果你在运营其它类型的中转后端，欢迎[提交 issue](../../issues) 评估接入。

## 如果你运营中转站

**可以将 LoongPort 作为你自己站点的客户端推荐给用户** —— 无需改动任何代码，用户填入你的域名即可接入。LoongPort 没有账号体系、没有服务端，不经手流量也不经手资金：用户注册的是你的站点、充值付给你，凭据仅存储在其本机的 SQLite 中。

它替你完成的三件事，每一条均可在源码中核验：`normalize_site_origin`（`relay/api.rs`，地址栏整条粘贴的域名亦可识别）、落至你自己注册页的自助注册（`relay/login.rs`，新站 `/register`、老用户 `/login`，邀请码随 URL 附带）、注入登录态的自助充值（`relay/purchase.rs`，打开 `{你的域名}/purchase`）。

收益、顾虑、技术前提与接入步骤见 **[loongport.dev/zh/for-relays](https://loongport.dev/zh/for-relays)**。

## 上游项目

**[cc-switch](https://github.com/farion1231/cc-switch)**（作者 [@farion1231](https://github.com/farion1231)，MIT）—— 本项目的基座，自 v3.19.1 fork，已合并上游至 v3.20.0；图标衍生自它，版权声明保留于 [LICENSE](LICENSE)。

cc-switch 是通用的多供应商管理器，管理所有 CLI 的所有供应商，还提供本地代理、MCP、Skills、Prompts 与会话管理。LoongPort 只做「用中转服务省钱运行 AI CLI」这一条链路。两者数据目录分离（`~/.cc-switch/` 与 `~/.loongport/`），可以同时安装、同时运行。

**[sub2api](https://github.com/Wei-Shaw/sub2api)**（LGPL-3.0）—— 多数中转站运行的后端。LoongPort 是它的**纯 HTTP 客户端**：不链接、不包含、也未复用其代码，仅依据其公开接口调用。LoongPort 为非官方客户端，与其作者无关联 —— 使用中遇到的问题请提交至本仓库。

## 从源码构建

需要 Node.js 22（见 `.node-version`）与 Rust 工具链（版本由 `rust-toolchain.toml` 锁定，rustup 会自动安装）。

```bash
git clone https://github.com/SailingLoong/LoongPort.git
cd LoongPort
pnpm install
pnpm dev           # 开发模式，前端热更新
```

打包命令分平台 —— `--bundles app` 出的是 macOS 的 `.app`，在 Windows 上没有作用：

```bash
# macOS
pnpm tauri build --bundles app

# Windows x64 NSIS Setup（应用内更新也使用同一种安装器）
pnpm tauri build --target x86_64-pc-windows-msvc --bundles nsis
```

macOS 上本机构建的应用不带隔离标记，Gatekeeper 不会介入。

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

见 [CONTRIBUTING.md](CONTRIBUTING.md)。修改中转站链路前请先阅读
[`LOONGPORT.md`](LOONGPORT.md)，其中记录了若干**看似有误、实则必须如此**的约束
（`model_provider` 为何必须是 `custom`、为何不能声明 `requires_openai_auth`、
退出 ChatGPT 为何按 bundle id）。每条约束均有测试钉住。

## 许可证

[MIT](LICENSE)。
