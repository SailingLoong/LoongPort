# Changelog

All notable changes to LoongPort are documented in this file.

本文件记录 LoongPort 的所有重要变更。

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Version numbers continue upstream's series (see Provenance at the bottom), so
LoongPort's first release is 3.19.2 rather than 0.1.0.

格式遵循 [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)。版本号延续上游的序列
（见文末「溯源」），所以 LoongPort 的第一个版本是 3.19.2 而不是 0.1.0。

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
