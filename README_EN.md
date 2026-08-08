<div align="center">

<img src="assets/branding/loongport-icon-master.png" alt="" width="96" height="96">

# LoongPort

### Codex at 5% of official cost, Claude at 20%, no extra network setup

[![Download](https://img.shields.io/github/v/release/SailingLoong/LoongPort?label=Download&color=2ea44f&style=for-the-badge)](../../releases/latest)

[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS-lightgrey.svg)](../../releases)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-orange.svg)](https://tauri.app/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

### 🌐 Official website: **[loongport.dev](https://loongport.dev)**

[中文](README.md) | English

</div>

## What it does

You want to run Codex CLI or Claude Code without paying official API rates. Normally
that means: sign up with a relay provider, hunt for their console, create an API key by
hand, copy the `base_url` correctly, track down the config file, and get every field
exactly right. Then do it all again for another CLI or tier.

LoongPort collapses that into two steps — **enter a domain, sign in once.** It
provisions a key for every tier your account can reach, writes each CLI's config in
its own shape, and switching tiers becomes a single click.

If your site has an image tier, you can also
[**generate images right inside your CLI**](#generating-images-in-your-cli) — without
giving up the tier you chat on.

<div align="center">
  <img src="assets/screenshots/main-zh.png" alt="LoongPort main window: the relay list, each row showing its balance and tier count" width="820">
  <br>
  <sub>Chinese UI shown; the app also ships English, Traditional Chinese and Japanese.</sub>
</div>

## Three minutes to first run

> **If you run a relay**: feel free to forward this section to your users. They do **not**
> need cc-switch and do **not** need an account with us — going from download to a working
> CLI is the four steps below, and they only ever deal with **your** site.
> To have them land on your site by default, see [If you run a relay](#if-you-run-a-relay).

1. **Download and open it** — see [Install](#install). On first launch you get a
   "pick a service site" dialog.
2. **Paste the relay's domain** — copying straight from your browser's address bar works
   (`https://bestapi.store/usage` and the like; any trailing path is stripped for you).
3. **Register or sign in** — LoongPort opens **that site's own** registration page.
   If you already have an account, a banner at the top takes you to the sign-in page in
   one click. The whole thing happens on the site's real pages; LoongPort receives the
   post-login credentials and **never handles your password**.
4. **Done** — every tier your account can use already has a key, and the configs are
   written. From there:
   - **Switch tiers**: one click on **Enable**
   - **Top up**: the button next to the balance opens the site's own payment page
   - **Generate images** (when the site has image tiers): see
     [Generating images in your CLI](#generating-images-in-your-cli)

No config files to edit, no API keys to create by hand, no `base_url` to remember.

## If you run a relay

You can hand LoongPort to your users as **a client for your own site** — it is a generic
sub2api client and is not tied to any particular relay:

- **It replaces your setup guide.** Users no longer follow a tutorial to create a key,
  copy a `base_url`, and hunt down a config file. The four steps above are the whole
  thing, and three of them happen on your site.
- **It does not come between you and your users.** LoongPort has no accounts and no
  server. Users register on **your** site, top up through **your** payment page, and
  their credentials never leave their own machine (SQLite under `~/.loongport/`).
- **To have users land on your site by default**: LoongPort fetches a signed remote
  config that can carry a "recommended sites" list — it appears at the top of the
  "pick a service site" screen, one click to connect. Just [open an issue](../../issues).
- **Affiliate and promo codes are supported too**: the same config can carry your
  referral code and a registration promo code, applied automatically at sign-up.

## Why it costs so much less

Two discount layers — Codex gets both, Claude gets one:

| Factor | | Who gets it | Why |
|---|---|---|---|
| Tier multiplier | **×0.1** | Codex only | Most GPT tiers bill at a 0.1 multiplier — a tenth of the official rate for the same token usage. Anthropic tiers carry no such discount and bill at 1 |
| Currency convention | **×1/6.7** | Codex and Claude alike | Relay sites generally price 1 CNY as if it were 1 USD, while the actual rate is roughly 6.7 CNY to the dollar |

So Codex lands at `0.1 × 1/6.7 ≈ 0.015` — about **1.5% of the official API cost** — and
Claude at `1 × 1/6.7 ≈ 0.15`, about **15% of official cost**. The headline says "5%" and
"20%" because multipliers vary by tier and by site, so there is margin built in. Full
derivation with caveats:
**[loongport.dev/en/pricing](https://loongport.dev/en/pricing)**

LoongPort is free. It never handles your payment and takes nothing out of your balance.
You top up with the relay provider.

> Registration links carry our referral code, which may earn us a rebate from the relay
> site; `bestapi.store` is our own site, and the built-in `LOONGPORT` promo code is a
> new-user credit there. Neither affects your price, and you can use any domain. Both
> tables live in `src-tauri/src/relay/` — `aff.rs` and `promo.rs`.

## Your official accounts stay untouched

Both official logins stay where they are — switch back any time without editing anything
by hand. The mechanism differs between the two:

- **Codex**: the ChatGPT desktop app and the `codex` CLI **share one** credentials file
  (`~/.codex/auth.json`), so it has to be preserved explicitly — LoongPort does that by
  default and never writes to it when switching tiers.
- **Claude**: the official login lives in `~/.claude/.credentials.json` while tier
  switches write `settings.json` next to it — **two separate files**, so there is nothing
  to overwrite.

## Install

| Platform | Requirement |
|---|---|
| **Windows** | Windows 10 or later (needs the WebView2 runtime, which Windows 10 and later generally ship with) |
| **macOS** | macOS 12 (Monterey) or later |

Download from the [Releases](../../releases) page. Every release is built automatically
by GitHub Actions:

| Platform | File | Notes |
|---|---|---|
| **Windows** | `…-Windows-Portable.zip` | **Recommended** — unzips to a single exe; no install step, so there is nothing for security software to block |
| | `…-Windows.msi` | Installer, adds a Start menu entry |
| **macOS** | `…-macOS.dmg` | Universal binary |

On ARM64 Windows machines (Snapdragon laptops and the like), use the two files with
`-arm64` in the name.

> **macOS blocks the first launch.** The build is not signed or notarized by Apple, so
> Gatekeeper reports it as "damaged" — it is not damaged, it is unsigned. **Drag it into
> Applications first, do not open it**, then run this once in Terminal:
>
> ```bash
> xattr -dr com.apple.quarantine /Applications/LoongPort.app
> ```
>
> It opens normally after that, and you only do this once. What that command does, why it
> is safe, and how to handle the "could not set file security" error when upgrading are
> all covered at **[loongport.dev/en/download](https://loongport.dev/en/download)**.

## How it works

1. **Enter a domain** — your relay provider's site. Pasting straight from the address bar
   works; leave it blank to use the default.
2. **Sign in** — the provider's real login page loads in a window; register or sign in
   right there. LoongPort receives the resulting credentials and never sees your password.
3. **Keys provisioned** — one per available tier. Existing keys with matching names are
   reused before new ones get created, so hitting refresh never litters your account.
4. **Click the tier you want** — Codex uses OpenAI tiers, Claude uses Anthropic tiers.
   One click writes the matching config (`~/.codex/config.toml`, or Claude's settings),
   and Codex or Claude Code just works from there.

> **When switching a Codex tier**, the ChatGPT desktop app is quit and reopened for you —
> it only reads its config at startup, so without a restart the new tier has no effect.
> **On macOS it asks the app to quit**: if ChatGPT has a conversation in progress it shows
> its own confirmation dialog and you can cancel (which aborts that switch). **On Windows
> the process is force-terminated**, with no dialog, so the app warns you before switching.
> Claude tier switches do not involve it.

Credentials and site data live in a local SQLite database under `~/.loongport/` and are
sent only to the relay site you chose, as the Bearer token on its API calls. LoongPort
has no account system and no server of its own, so it never receives them.

## Generating images in your CLI

Tiers that only serve image models are collected on their own **Codex Images** tab
(next to Codex). Click **Enable** on one of them, and asking for an image in
conversation just works in Codex, Claude Code or Gemini CLI — the generation runs
through LoongPort's built-in image tool (an MCP server).

Four things worth knowing:

- **This tab is not usable on its own** — it works alongside a chat tier. All it
  decides is which tier your images come from.
- **Your chat tier does not step aside.** Chat goes to `/v1/responses` with whichever
  tier you picked on the Codex page; images go to `/v1/images/generations` with
  whichever you picked here — two independent "current" selections, and switching one
  never disturbs the other. So you can chat through DeepSeek while images come from a
  relay's 4K tier.
- **Switching image tiers needs no CLI restart.** The choice lives in LoongPort's own
  database and is read fresh on every generation; the CLI's config file is not touched.
  Only the **very first** image tier needs a new terminal (that is when the tool is
  added to the CLI's config, and CLIs only read their config at startup).
- **A site with no image group leaves this tab empty**, and your CLI configs are left
  untouched. Picking between a 1K and a 4K tier is a spending decision, so LoongPort
  never picks for you.

## Updating

**The installer (msi) and the macOS build check for updates on their own**: a few
seconds after launch they ask once in the background, and a newer version shows up
under Settings → About — one click downloads, installs and restarts. A failed check
never bothers you (being offline or unable to reach GitHub is common enough), and you
can press "check for updates" yourself at any time.

> **The Windows portable build does not update in place** — it cannot replace itself
> while running. It still tells you a new version exists, but you download the new zip
> from [Releases](../../releases). That is the trade-off for the portable build: no
> install step, so nothing for security software to block.

## What it supports

| | Shipped | In progress |
|---|---|---|
| **Relay services** | sub2api | new-api |
| **AI CLIs** | codex · claude | gemini · grok |
| **Platforms** | macOS · Windows | Linux |

> **The "AI CLIs" row is about chat tiers.** The image tool registers with codex,
> claude **and gemini** — "gemini in progress" means it cannot yet be the target of a
> chat tier (its config shape is not written yet), not that it is untouched.

> **"In progress" is not a wishlist.** Take new-api: the login identifier is already
> modelled as a neutral `login_identifier` (rather than hardcoding sub2api's field name),
> and the `platform_map` table is complete — what is missing is the adapter for its own
> API surface. **If you run a new-api site, or you are its developer,
> [opening an issue](../../issues) would move this along considerably**: what we need is a
> site to test against and confirmation of a few endpoint shapes. The same goes for any
> other relay backend.

You can point it at your own site domain; a working one is preset by default. macOS and
Windows have the same feature set.

## If you run a relay service

**You can hand this to your users as the client for your own site** — no code changes
needed, a user typing your domain already works. LoongPort has no account system and no
server of its own, and handles neither the traffic nor the money: users register with you
and pay you, and credentials live in a local SQLite database on their own machine.

Three things it takes off your plate, each checkable in the source: `normalize_site_origin`
(`relay/api.rs` — a domain pasted straight from the address bar is accepted),
self-service signup on your own registration page (`relay/login.rs` — a fresh site
lands on `/register`, a returning one on `/login`, referral codes riding along in the URL),
and self-service top-up with the session injected (`relay/purchase.rs` — it opens
`{your-domain}/purchase`).

The benefits, the concerns, the technical prerequisites and how to get on board are all at
**[loongport.dev/en/for-relays](https://loongport.dev/en/for-relays)**.

## Upstream projects

**[cc-switch](https://github.com/farion1231/cc-switch)** (by
[@farion1231](https://github.com/farion1231), MIT) — the base this is built on, forked
at v3.19.1 and merged upstream through v3.19.2. The icon is derived from theirs and
their copyright notice is kept in [LICENSE](LICENSE).

cc-switch is a general multi-provider manager covering every CLI and every provider,
plus proxy mode, MCP, Skills, Prompts and Session Manager. LoongPort does exactly one
path: running an AI CLI cheaply through a relay service. The two install side by side
with separate data directories (`~/.cc-switch/` vs `~/.loongport/`) and can run at the
same time.

**[sub2api](https://github.com/Wei-Shaw/sub2api)** (LGPL-3.0) — the backend most relay
sites run. LoongPort is a **plain HTTP client** of it: it does not link against, bundle,
or reuse any of its code, only calls its documented interfaces. Not an official client
and not affiliated with its author — report problems you hit with LoongPort here.

## Build from source

Requires Node.js 22 (see `.node-version`) and the Rust toolchain (version pinned by
`rust-toolchain.toml`; rustup installs it automatically).

```bash
git clone https://github.com/SailingLoong/LoongPort.git
cd LoongPort
pnpm install
pnpm dev           # dev mode, hot reload
```

Packaging differs per platform — `--bundles app` produces a macOS `.app` and does
nothing useful on Windows:

```bash
# macOS
pnpm tauri build --bundles app

# Windows — both flags are required. Bare `pnpm tauri build` fails at the MSI link
# step (WiX ICE38) because of this repo's per-user WiX template, and `bundle.targets`
# is "all", so it would also try to fetch the NSIS toolchain.
pnpm tauri build --target x86_64-pc-windows-msvc --bundles msi
```

On macOS, a build produced on your own machine carries no quarantine flag, so
Gatekeeper stays out of the way.

<details>
<summary><strong>Tech stack and tests</strong></summary>

**Frontend**: React 18 · TypeScript 5 · Vite 7 · TailwindCSS 3.4 · TanStack Query v5 · shadcn/ui

**Backend**: Tauri 2.8 · Rust (edition 2021, version pinned by `rust-toolchain.toml`) · serde · tokio · SQLite

```bash
cargo test --manifest-path src-tauri/Cargo.toml   # backend
pnpm vitest run                                   # frontend
pnpm tsc --noEmit                                 # type check
```

</details>

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Before touching the relay-account path, read
[`LOONGPORT.md`](LOONGPORT.md) — it documents constraints that look wrong until you know
why (`model_provider` must be `custom`, `requires_openai_auth` must be absent, quitting
ChatGPT goes by bundle id). Each has a test pinning it.

## License

[MIT](LICENSE).
