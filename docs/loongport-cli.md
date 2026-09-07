# loongport-cli：无界面版使用指南

> **这是什么**：LoongPort 的**无 UI 版本**（纯命令行，一次性配置工具）。给两类人用：
>
> 1. **纯服务器**（没有桌面环境的 Linux 机器）
> 2. **老发行版**：Ubuntu 20.04 及更早、CentOS 等装不上桌面版的系统
>    （桌面版要求 webkit2gtk-4.1 与较新的 glibc，Ubuntu 22.04 以下的官方源没有）
>
> 桌面版用户（Ubuntu 22.04+ / Windows / macOS）不需要它——直接用[桌面版](../../releases)即可。

## 安装（三步，约 10 秒）

静态编译单文件，**零依赖**，任何 x86_64 Linux 都能直接跑（Ubuntu 20.04/22.04/24.04、Debian、CentOS、Alpine 通吃），不需要安装任何库。

```bash
# 1. 下载（去 Release 页拿最新版的 tar.gz，资产名带版本号）
curl -LO https://github.com/SailingLoong/LoongPort/releases/download/v6.17.1/LoongPort-CLI-v6.17.1-Linux-x86_64.tar.gz

# 2. 解压（得到单个可执行文件 loongport-cli）
tar xzf LoongPort-CLI-v6.17.1-Linux-x86_64.tar.gz

# 3. 运行
./loongport-cli --version
```

想全局可用就挪进 PATH：

```bash
sudo mv loongport-cli /usr/local/bin/
```

说明：

- 国内服务器连 GitHub 不畅时，先在能访问的机器上下载，再传上去（`scp` / `rsync` 均可）
- 需要走代理时，识别标准的 `https_proxy` 环境变量

## 使用：一条命令完成配置

```bash
loongport-cli --add-site <站点域名或完整网址> --key <sk-密钥> [--app <CLI>] [--model <模型id>]
```

| 参数 | 说明 |
|---|---|
| `--add-site` | 中转站地址，裸域名（`example.com`）或完整网址都行 |
| `--key` | 站点的 sk 密钥 |
| `--app` | 要配置的 CLI：`codex`（默认）/ `claude` / `gemini` / `grok` / `opencode` / `openclaw` / `hermes` |
| `--model` | 模型 id，可省略（见下） |

### 实际例子（最常见：服务器上配 codex）

```console
$ loongport-cli --add-site https://example.com --key sk-xxxx --app codex
探测站点 https://example.com …
✅ 已为 codex 配置「某中转站」（模型 gpt-5.4）
   密钥已写进 CLI 自己的配置文件，无需再设环境变量。
   验证: codex exec "回复一个字：好"
```

### `--model` 省略时会发生什么

工具会去拉站点的 `/v1/models` 模型列表：

- **在终端里**（交互环境）：列出编号清单，输序号选择，直接回车默认第 1 个
- **在脚本里**（非交互，如 `ssh` 远程执行）：列出可用模型后退出，提示带 `--model <id>` 重新运行——保证可脚本化
- **拉取失败**（密钥无效等）：提示可用 `--model` 显式指定绕过

其他：`--help` 看完整用法，`--version` 看版本。`claude-desktop`（桌面应用）与 `codex-image`（依赖图形界面）会被明确拒绝并给出指引。

## 它把配置写到了哪

只写目标 CLI 自己的配置文件（`$HOME` 下），不碰其他任何东西：

| CLI | 写入位置 | 内容 |
|---|---|---|
| codex | `~/.codex/config.toml` | `base_url` 按协议约定自动带 `/v1`、`wire_api = "responses"`、密钥进 `experimental_bearer_token`（**不动 `auth.json`**，不影响已有 OAuth 登录态） |
| claude | `~/.claude/settings.json` | env 块：`ANTHROPIC_BASE_URL`（站点根，不带 `/v1`）、`ANTHROPIC_AUTH_TOKEN`、三个角色别名（haiku/sonnet/opus）指向所选模型 |
| 其余 | 各 CLI 的约定位置 | 同理，与桌面版切档写入同一批函数 |

配完直接用目标 CLI 验证：

```bash
codex exec "回复一个字：好"    # 或
claude -p "回复一个字：好"
```

## 前提与边界

- **前提**：目标 CLI 本身（codex / claude 等）需要自己先装好，本工具只负责写配置
- **重复运行**：固定 provider id `loongport-relay`，再跑一次 = 覆盖上一次。换站、换 key、换模型都是重跑同一条命令，不会堆积残留
- **不做的事**：浏览器 OAuth 登录、多账号、档位管理、本地路由与故障转移——那些是桌面版的能力；服务器上的多站多档位需求等真实用户反馈再议
- 退出码：`0` 成功，`1` 运行错误（网络 / 密钥 / 写盘），`2` 参数错误

## 与桌面版的关系

与桌面版共享同一套站点探测与配置写入逻辑（同一份代码，唯源不分叉），但互不共享数据：服务器上用 loongport-cli 配好的文件，不影响你桌面机器上的 LoongPort。
