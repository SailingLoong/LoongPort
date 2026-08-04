# Changelog

All notable changes to LoongPort are documented in this file.

本文件记录 LoongPort 的所有重要变更。

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Version numbers continue upstream's series (see Provenance at the bottom), so
LoongPort's first release is 3.19.2 rather than 0.1.0.

格式遵循 [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)。版本号延续上游的序列
（见文末「溯源」），所以 LoongPort 的第一个版本是 3.19.2 而不是 0.1.0。

## [3.20.0] — 2026-08-05

### Added / 新增

- **Image generation inside your CLI, without giving up a chat tier.** Tiers that
  only serve image models now carry an **Use for images** button. Click it and
  LoongPort registers a built-in MCP server with Codex, Claude Code and Gemini
  CLI — after that, "generate an image" just works in conversation.

  **Two independent "current" selections.** Which tier you chat on and which tier
  images come from are separate choices that don't interfere: chat goes to
  `/v1/responses` with the active tier's key, images go to
  `/v1/images/generations` with the image tier's key. Switching one never
  disturbs the other.

  **Switching image tiers needs no CLI restart.** The MCP entry stores no tier
  id; the choice lives in LoongPort's database and is read fresh on every
  generation. Only the very first activation needs a new terminal (that's when
  the MCP entry is added, and CLIs only read their config at startup).

  **Nothing is pushed on you.** A relay with no image group shows no image UI at
  all and your CLI configs are left untouched. Picking between a 1K and a 4K tier
  is a spending decision, so LoongPort never picks for you.

  **CLI 里生图，不必让出对话档位。** 只提供生图模型的档位上多了一个「启用生图」按钮。
  点一下，LoongPort 就把内置的生图工具注册进 Codex / Claude Code / Gemini CLI，
  之后在对话里说「生成一张图」即可。

  **两个各自独立的「当前项」。** 「用哪个档位对话」与「图从哪个档位出」是两个互不干扰的
  选择：对话走 `/v1/responses` 用当前档位的密钥，生图走 `/v1/images/generations` 用生图
  档位的密钥。切换任一个都不影响另一个。

  **换生图档位不用重启 CLI。** MCP 条目里不存档位 id，选择存在 LoongPort 自己的库里、
  每次生图时现读。只有第一次启用需要新开终端（那时才往配置里新增条目，而 CLI 只在启动时
  读配置）。

  **不主动塞给你。** 没有生图分组的中转站完全不显示生图相关的东西，你的 CLI 配置一个字
  不动。而 1K 与 4K 之间怎么选是花钱的决定，LoongPort 不替你选。

- **Connectivity check now tells you what a tier can actually serve.** The check
  used to answer only "is the host reachable" — which reports *healthy* for a
  tier whose key has expired, that serves no models at all, or that serves only
  image models while being used for chat. It now also asks the tier what it can
  serve (a listing endpoint, so it costs nothing and stays a button you can press
  freely) and says so: `6 models (…)`, `image generation only — not usable for
  chat`, or `key has expired (401)`.

  **连通检测现在会告诉你这个档位到底能调什么。** 原来它只回答「主机通不通」——
  而密钥已过期、分组没挂任何模型、或只挂了生图模型却被当对话档位用，这三种它都报
  「连通正常」。现在它会额外问一句这个档位能提供什么（列表接口，零成本，所以仍是一个
  可以随手按的按钮），并如实说出来：`6 个模型（…）`、`只能生图，不能对话`、
  `密钥已失效（401）`。

### Fixed / 修复

- **Image-only tiers were provisioned with a text model, so selecting one 404'd.**
  Provisioning now asks each group which models it actually serves and writes the
  group's real `gpt-image-*` name when it serves nothing else — picking the newest
  generation by numeric version, not lexical order. Existing tiers are corrected
  on the next refresh, but only when their config is still untouched, so a tier
  you edited yourself is left alone.

  纯生图分组原先被写入了文本模型名，选中即 404。现在 provision 会问每个分组它真正提供
  哪些模型，只挂生图模型的分组就写它自己的 `gpt-image-*`（按版本号数值取最新的一代，
  不是字典序）。已存在的档位在下次刷新时会被修正 —— 但仅限配置未被改动过的，
  你自己编辑过的档位不会被动。

- **"Restore default config" no longer breaks an image tier.** It used to write the
  text default back, i.e. the button meant for un-bricking a tier bricked image
  ones instead.

  「恢复默认配置」不再弄坏生图档位 —— 它原先会把文本默认值写回去，也就是专门用来救砖的
  按钮反过来把生图档位弄砖了。

- **A custom data directory is now honoured by the image tool.** It used to read
  the default `~/.loongport` regardless, so anyone who had moved their data
  directory got "database not found" or a stale tier — silently, in both cases.

  自定义数据目录现在对生图工具也生效了。它原先一律读默认的 `~/.loongport`，
  所以挪过数据目录的用户会遇到「找不到数据库」或读到过期档位 —— 两种都不报错。

- **Tauri's npm and crate versions had drifted apart, so no platform could be
  packaged.** `@tauri-apps/api` is pinned back to the crate's minor. This had been
  broken since the `tauri 2.11.1` dependency bump; CI does not run `tauri build`,
  and Tauri only checks this pairing at package time.

  Tauri 的 npm 包与 crate 版本已经漂开，导致两个平台都打不出包。`@tauri-apps/api` 已对回
  crate 的 minor。这个问题自 `tauri 2.11.1` 那次依赖升级起就存在 —— CI 不跑
  `tauri build`，而 Tauri 只在打包时校验这一对版本。

## [3.19.2] — 2026-08-04

First LoongPort release. What follows is what this fork adds on top of cc-switch
v3.19.2; the inherited base is not re-listed.

LoongPort 的首个版本。以下是本 fork 在 cc-switch v3.19.2 之上新增的部分，继承来的
基座功能不再重复列出。

### Added / 新增

- **Relay-account automation — one domain, one sign-in.** Enter a relay
  provider's domain and sign in once; LoongPort provisions an API key for every
  tier the account can reach and writes each CLI's config in its own shape
  (`~/.codex/config.toml` for Codex, `settings.json` for Claude Code). Switching
  tiers afterwards is a single click. Existing keys with matching names are
  reused before new ones are created, so refreshing never litters the account.

  **中转账号全自动 —— 填一个域名、登录一次。** 填中转站域名并登录一次，LoongPort 就为
  账号可用的每个档位备好 API Key，并按各 CLI 的形状写好配置（Codex 是
  `~/.codex/config.toml`，Claude Code 是 `settings.json`）。之后换档位只要点一下。
  先复用名字匹配的已有 key，找不到才新建，所以反复刷新不会在账号里堆垃圾 key。

- **Official-site direct connection (vendor path).** An account can also be a
  vendor's own platform — DeepSeek is the first — where LoongPort signs in,
  provisions a key through that platform's own API, and writes it to every CLI
  that can use it.

  **官网直连（vendor 路径）。** 账号也可以是厂商自己的平台（首个是 DeepSeek）：
  LoongPort 登录后经该平台自身的 API 建 key，再写进所有能用它的 CLI。

- **Balance display with a low-balance warning.** Each account row shows its
  current balance, with an in-app amber warning below $5. Scoped to relay
  accounts only — vendor balances differ in both currency and type.

  **余额展示与低余额提醒。** 每个账号行显示当前余额，低于 $5 时在应用内给琥珀色提醒。
  只对中转站账号生效 —— 官网直连的余额币种与类型都不同。

- **Signed remote configuration.** Sponsored-operator presets and referral codes
  can be updated without shipping a new build. The payload is Ed25519-signed and
  verified before use, with a three-level fallback; the private key never enters
  the repository.

  **带签名的远端配置。** 赞助运营商列表与邀请码可以不发新版本就更新。载荷经 Ed25519
  签名、使用前先验签，并有三层回落；私钥不进仓库。

- **Portable Windows build.** Releases carry a `-Windows-Portable.zip` next to
  the MSI — it unzips to a single `LoongPort.exe` with the WebView2 loader
  statically linked, so no extra DLLs sit beside it and there is no install step
  for security software to block.

  **Windows 免安装版。** 发布产物在 MSI 之外多一个 `-Windows-Portable.zip`：解压出单个
  `LoongPort.exe`，WebView2 loader 已静态链接，同目录不需要额外 DLL，也没有会被安全
  软件拦住的安装步骤。

### Changed / 变更

- **Separate identity and data directory.** Data lives in `~/.loongport/`
  (`loongport.db`), fully isolated from an installed cc-switch at
  `~/.cc-switch/`; both can be installed and run at the same time. The deep-link
  scheme is `loongport://`.

  **独立身份与数据目录。** 数据在 `~/.loongport/`（`loongport.db`），与已装的 cc-switch
  的 `~/.cc-switch/` 完全隔离，两者可同时安装、同时运行。deeplink scheme 是
  `loongport://`。

- **Pricing framing states cost, not savings.** User-facing copy says "Codex at
  5% of official cost, Claude at 20%" rather than "saves 95%" — the derivation
  and its caveats are on the pricing page.

  **省钱口径改为「只花百分之几」。** 用户可见文案是「Codex 只花官方的 5%、Claude 只花
  20%」而不是「省 95%」—— 推导与说明在定价页。

- **Both commercial relationships disclosed in the README.** Registration links
  carry a referral code from a compile-time table
  (`src-tauri/src/operator/aff.rs`), and one preset site — the only entry in the
  built-in promo-code table (`operator/promo.rs`) — is run by the maintainer.
  Both tables are compiled into the binary and visible in the source. Neither
  affects the user's price and nothing is deducted from their balance.

  **README 里把两层商业关系都说清。** 一是注册链接带编译期常量表里的邀请码
  （`src-tauri/src/operator/aff.rs`）；二是有一个预置站点由维护者自己运营，它也是内置
  优惠码表（`operator/promo.rs`）里唯一的一条。两张表都编译进二进制、源码里看得到。
  两者都不影响用户的价格，也不从余额里扣。

### Removed / 移除

- **Automatic updates.** Upstream's updater endpoint would have upgraded
  LoongPort users into cc-switch, so `plugins.updater` is removed outright rather
  than pointed elsewhere.

  **自动更新。** 上游的 updater 端点会把 LoongPort 用户升级成 cc-switch，所以
  `plugins.updater` 整块删掉，而不是改指到别处。

- **Upstream's changelog, release notes, user manual and guides.** They document
  cc-switch's own feature set (local proxy, MCP, Skills, Prompts, session
  manager) and cite its issue numbers — which resolve to unrelated issues in this
  repository. See Provenance for where that history now lives.

  **上游的 changelog、release notes、用户手册与指南。** 它们记的是 cc-switch 自己的功能面
  （本地代理、MCP、Skills、Prompts、会话管理），且引用它的 issue 编号 —— 那些编号在本仓
  会指向无关的 issue。那段历史现在在哪见「溯源」。

- **Upstream's Flatpak manifest and two orphaned release scripts.** The Flatpak
  files carried cc-switch's app id, name and feature list, and no build step ever
  invoked them. The two scripts served an upstream website and the updater this
  fork removed; nothing called either.

  **上游的 Flatpak 清单与两个孤立的发布脚本。** Flatpak 那几个文件带的是 cc-switch 的
  app id、名字与功能列表，而且从来没有构建步骤调用它们；两个脚本服务的是上游官网与本
  fork 已删掉的 updater，两者都无人调用。

### Fixed / 修复

- **The About screen said "CC Switch".** Settings → About hardcoded the upstream
  name next to the LoongPort icon and version — the one screen a user opens to
  answer "what am I running". Twenty-odd further strings across all four locales
  said it too, and two of those also pointed at `~/.cc-switch/` for skills and
  backups, a directory this app never writes to.

  **「关于」那一屏写着 CC Switch。** 设置 → 关于把上游的名字硬编码在 LoongPort 图标与
  版本号旁边 —— 而那正是用户打开来回答「我在运行什么」的地方。四种语言里另有 20 多处
  同样问题，其中两处还把技能与备份目录指向 `~/.loongport/` 之外的 `~/.cc-switch/`，
  那是本应用从不写入的位置。

- **"No tiers here" no longer looks the same as "the fetch failed".** Provisioning
  probes every platform at once and each tier lands on the CLI its own platform
  maps to, so a site with no Anthropic tiers leaves the Claude tab empty — which
  read identically to a genuine fetch failure. Retrying is pointless in the first
  case and worthwhile in the second, so the two now say different things.

  **「分组落在别的平台」不再和「拉取失败」长得一样。** provision 一次探全部平台，每个
  分组按自己的平台落到对应 CLI ⇒ 某个站没有 anthropic 分组时 claude 那一屏是零档位，
  而它与真的拉失败此前显示同一句话。前者再点一百次也不会有（该切 tab 或换站），后者
  重试有意义，所以两种处境现在说的是两句话。

- **A user's own provider could be mistaken for a managed one.** The frontend
  decided "did we generate this record" by matching an id prefix — a shape test
  standing in for a provenance question. Live-config import writes provider ids
  taken straight from the user's own CLI config, so a colliding prefix was
  genuinely reachable. The check now goes by provenance.

  **用户自己的 provider 可能被误判成托管项。** 前端判「这条记录是不是我们生成的」靠的是
  id 前缀 —— 那判的是形状，而问题问的是来源。live config 导入的 provider id 直接取自
  用户自己的 CLI 配置，所以撞上前缀是真实可达的。判据改为按来源判。

- **The delete confirmation said untrue things about a never-signed-in row.** It
  claimed the sign-in state would be removed and could be restored by signing in
  again — wrong for a row that had never been signed into. Split into two
  wordings chosen by state.

  **删除确认对「从没登录过」的行说错话。** 原文案说「会删掉登录态…重新登录就能再用」，
  而那一行可能从没登录过。按状态拆成两条文案。

---

## Provenance / 溯源

LoongPort was forked from [cc-switch](https://github.com/farion1231/cc-switch)
v3.19.1 (MIT, by [@farion1231](https://github.com/farion1231)) and has merged
upstream through v3.19.2. **Most of the code in this repository is theirs.**

**The history before 3.19.2 is upstream's and is documented upstream** — see
[cc-switch's CHANGELOG](https://github.com/farion1231/cc-switch/blob/main/CHANGELOG.md).
This file starts where LoongPort starts.

LoongPort 在 [cc-switch](https://github.com/farion1231/cc-switch) v3.19.1（MIT，作者
[@farion1231](https://github.com/farion1231)）上 fork，之后合并上游至 v3.19.2。
**这个仓里绝大部分代码是它的。**

**3.19.2 之前的历史属于上游，也记在上游** ——
见 [cc-switch 的 CHANGELOG](https://github.com/farion1231/cc-switch/blob/main/CHANGELOG.md)。
本文件从 LoongPort 自己开始记。
