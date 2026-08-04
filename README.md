<div align="center">

<img src="assets/branding/loongport-icon-master.png" alt="" width="96" height="96">

# LoongPort

### 同样的 Codex，成本省 95% 以上，国内直连

[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS-lightgrey.svg)](../../releases)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-orange.svg)](https://tauri.app/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

### 🌐 官方网站：**[loongport.dev](https://loongport.dev)**

中文 | [English](README_EN.md)

</div>

## 它替你做什么

你想用 Codex CLI，但不想按官方 API 的价付钱。照常规做法这意味着：去中转站注册、
找到控制台、手建一把 API Key、把 `base_url` 抄对、翻出 `~/.codex/config.toml`、
把 `wire_api` 和 `model_provider` 写对。想换个档位，再来一遍。

LoongPort 把这些压成两步 —— **填一个域名，登录一次。** 它会为你账号能用的每个档位
备好 key、把配置文件写好，之后换档位就是点一下。

## 为什么便宜这么多

两件事叠加：

| 因素 | | 为什么 |
|---|---|---|
| 档位倍率 | **×0.1** | 多数 GPT 档位的计费倍率是 0.1 —— 同样的 token 用量，按官方价的一折计费 |
| 汇率口径 | **×1/6.7** | 中转站普遍按「1 人民币抵 1 美元」计价，而实际汇率约 1 美元兑 6.7 人民币 |

`0.1 × 1/6.7 ≈ 0.015` —— 成本约为**官方 API 的 1.5%**，即省下约 98.5%。这里写
「95% 以上」是因为倍率随档位和站点浮动，留了余量。完整推导与几点说明：
**[loongport.dev/zh/pricing](https://loongport.dev/zh/pricing)**

LoongPort 本身免费，不经手你的付款、也不从你的余额里抽成 —— 它只是替你把配置写对。
你充值给的是中转服务商。

> **一层关系先说清**：注册链接会带上我们的邀请码（`src-tauri/src/operator/aff.rs`
> 里的编译期常量表，源码里看得到），我们可能因此从中转站获得返利。**这不影响你的
> 价格，也不会从你的余额里扣** —— 但你有权知道它存在。

## 不占用官方账号

ChatGPT 桌面版与命令行 `codex` 共用同一份凭据文件（`~/.codex/auth.json`）。
LoongPort 默认保留它，切换档位时不会写它 —— 所以你的官方订阅还在原处，想用随时
切回去，不必手改任何东西。

## 安装

| 平台 | 要求 |
|---|---|
| **Windows** | Windows 10 或更高 |
| **macOS** | macOS 12（Monterey）或更高 |

到 [Releases](../../releases) 页面下载：

- **Windows**：`LoongPort-v{version}-Windows.msi`（安装版）或 `-Windows-Portable.zip`（绿色版）
- **macOS**：`LoongPort-v{version}-macOS.dmg`

> **macOS 首次打开会被拦一下。** 还没做 Apple 代码签名与公证（需要 Apple 开发者
> 账号，正在办），所以 Gatekeeper 会报「已损坏」—— **不是真的损坏**。终端里执行
> 一次即可：
>
> ```bash
> xattr -dr com.apple.quarantine /Applications/LoongPort.app
> ```

> **Windows 装新版本时若报「无法设置文件安全性」。** 少数装了安全软件（如腾讯电脑
> 管家）的机器上，**覆盖安装**旧版本会失败：`could not set file security for file
> '...\Config.Msi\xxxxxxx.rbf'  Error: 5`。那是安全软件拦住了安装程序**备份旧文件**
> 的动作，**不是安装包坏了**。两个办法任选一个：
>
> 1. **先卸载旧版本再装**（设置 → 应用 → 卸载，然后双击新安装包）。全新安装不需要
>    备份旧文件，就不会被拦。**账号和配置不会丢** —— 它们在用户目录下，卸载不动它们。
> 2. **用绿色版**（`-Windows-Portable.zip`），解压即用，不经过 Windows 安装程序。
>
> 首次安装的用户不会遇到这个 —— 它只在覆盖旧版本时出现。

## 怎么用

1. **填域名** —— 你的中转站域名。不确定就留空，用默认的。
2. **登录** —— 弹窗里加载该站真实的登录页，注册或登录都在里面完成。LoongPort 拿到的
   是登录后的凭据，不经手你的密码。
3. **自动备 key** —— 每个可用档位一把。先复用你账号里名字匹配的，找不到才新建，
   所以反复点刷新不会在你账号里堆垃圾 key。
4. **选档位** —— 点一下就写好 `~/.codex/config.toml`。
5. **继续用** —— Codex 直接就能跑。它还会替你退出并重开 ChatGPT 桌面版 —— 那个 app
   启动时读配置、运行中改配置不会重新加载，所以这一步才是让新档位真正生效的关键。

凭据与站点信息存在本机 `~/.loongport/` 下的 SQLite 里，且**只发给你选的那个中转站**
（作为它 API 调用的 Bearer token —— 账号能用靠的就是这个）。LoongPort 自己没有账号
体系、没有服务端，拿不到你的凭据。

## 支持范围

| | 已支持 | 在做 |
|---|---|---|
| **中转服务** | sub2api | new-api |
| **AI CLI** | codex | claude · gemini · grok |
| **平台** | macOS · Windows | Linux |

站点域名可以自己填，默认预置一个可用的。macOS 与 Windows 功能一致。

> 一处细节差异：替你重启 ChatGPT 桌面版这一步，**macOS 上是「请求退出」**——
> 它有进行中的对话时会弹自己的确认框，你可以取消（那次切换随之中止）；
> **Windows 上是强制结束进程**，不会弹框，所以切换前应用会先告知你一次。

## 与 cc-switch 的关系

LoongPort 最初在 [cc-switch](https://github.com/farion1231/cc-switch) v3.19.1 上 fork，
之后合并上游至 v3.19.2，许可证同为 MIT —— 一个成熟的基座省掉了大量重复工作，
图标也衍生自它。

**两者做的是不同的事。** cc-switch 是通用的多供应商管理器，管所有 CLI 的所有供应商，
功能面大得多 —— 本地代理、MCP、Skills、Prompts、会话管理。LoongPort 只把「用中转
服务省钱跑 Codex」这一条链路做到全自动。想要那个全能工具就用 cc-switch。两者数据
目录分开（`~/.cc-switch/` 与 `~/.loongport/`），可以同时装、同时开。

## 从源码构建

需要 Node.js 20+ 与 Rust 工具链（1.85+）。

```bash
git clone https://github.com/SailingLoong/LoongPort.git
cd LoongPort
pnpm install
pnpm tauri dev     # 开发模式，前端热更新
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

**后端**：Tauri 2.8 · Rust（edition 2021，1.85+）· serde · tokio · SQLite

```bash
cargo test --manifest-path src-tauri/Cargo.toml   # 后端
pnpm vitest run                                   # 前端
pnpm tsc --noEmit                                 # 类型检查
```

</details>

## 参与贡献

欢迎提 issue 与 PR。动运营商那条链路之前请先读 [`LOONGPORT.md`](LOONGPORT.md) ——
里面记了几条**看着像写错、其实必须那样写**的地方（为什么 `model_provider` 必须是
`custom`、为什么不能声明 `requires_openai_auth`、为什么退 ChatGPT 要按 bundle id 而
不是进程名）。每条都有测试钉着，改错会当场报错而不是静默走歪。

## 许可证

[MIT](LICENSE) —— 继承自 cc-switch，其版权声明已保留。
