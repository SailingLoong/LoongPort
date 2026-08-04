<div align="center">

<img src="assets/branding/loongport-icon-master.png" alt="" width="96" height="96">

# LoongPort

### The same Codex — over 95% cheaper, no extra network setup

[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS-lightgrey.svg)](../../releases)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-orange.svg)](https://tauri.app/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

### 🌐 Official website: **[loongport.dev](https://loongport.dev)**

[中文](README.md) | English

</div>

## What it does

You want to run Codex CLI without paying official API rates. Normally that means:
sign up with a relay provider, hunt for their console, create an API key by hand,
copy the `base_url` correctly, track down `~/.codex/config.toml`, and get `wire_api`
and `model_provider` exactly right. Then do it all again to switch tiers.

LoongPort collapses that into two steps — **enter a domain, sign in once.** It
provisions a key for every tier your account can reach, writes the config file, and
switching tiers becomes a single click.

## Why it costs so much less

Two things compound:

| Factor | | Why |
|---|---|---|
| Tier multiplier | **×0.1** | Most GPT tiers bill at a 0.1 multiplier — a tenth of the official rate for the same token usage |
| Currency convention | **×1/6.7** | Relay sites generally price 1 CNY as if it were 1 USD, while the actual rate is roughly 6.7 CNY to the dollar |

`0.1 × 1/6.7 ≈ 0.015` — about **1.5% of the official API cost**, or roughly 98.5%
saved. This README says "over 95%" because multipliers vary by tier and by site, so
there is margin built in. Full derivation with caveats:
**[loongport.dev/en/pricing](https://loongport.dev/en/pricing)**

LoongPort itself is free. It never handles your payment and takes nothing out of your
balance — it only writes the config correctly. You top up with the relay provider.

> **One relationship, stated up front**: registration links carry our referral code
> (a compile-time table in `src-tauri/src/operator/aff.rs` — visible in the source),
> and we may earn a rebate from the relay site as a result. **This does not affect your
> price and nothing is deducted from your balance** — but you deserve to know it exists.

## Your official account stays untouched

The ChatGPT desktop app and the `codex` CLI share one credentials file
(`~/.codex/auth.json`). LoongPort preserves it by default and does not write to it
when switching tiers, so your official subscription is still there — switch back any
time without editing anything by hand.

## Install

| Platform | Requirement |
|---|---|
| **Windows** | Windows 10 or later |
| **macOS** | macOS 12 (Monterey) or later |

Download from the [Releases](../../releases) page:

- **Windows**: `LoongPort-v{version}-Windows.msi` (installer) or `-Windows-Portable.zip`
- **macOS**: `LoongPort-v{version}-macOS.dmg`

> **macOS blocks the first launch.** The build is not yet code-signed and notarized by
> Apple (that needs an Apple Developer account, which is in progress), so Gatekeeper
> reports the app as "damaged" — it is not. Run this once in Terminal:
>
> ```bash
> xattr -dr com.apple.quarantine /Applications/LoongPort.app
> ```

> **Windows: "could not set file security" when installing an update.** On a few
> machines running security software (Tencent PC Manager, for one), installing *over*
> an older version fails with `could not set file security for file
> '...\Config.Msi\xxxxxxx.rbf'  Error: 5`. That is the security software blocking the
> installer from **backing up the old files** — the package itself is fine. Either:
>
> 1. **Uninstall the old version first**, then run the new installer (Settings → Apps).
>    A fresh install has nothing to back up, so nothing gets blocked. **Your account
>    and settings are kept** — they live in your user folder, untouched by uninstall.
> 2. **Use the portable build** (`-Windows-Portable.zip`) — unzip and run, no Windows
>    Installer involved.
>
> First-time installs are unaffected; this only happens when overwriting an older version.

## How it works

1. **Enter a domain** — your relay provider's site. Leave it blank to use the default.
2. **Sign in** — the provider's real login page loads in a window; register or sign in
   right there. LoongPort receives the resulting credentials and never sees your password.
3. **Keys provisioned** — one per available tier. Existing keys with matching names are
   reused before new ones get created, so hitting refresh never litters your account.
4. **Pick a tier** — one click writes `~/.codex/config.toml`.
5. **Keep working** — Codex just works. The ChatGPT desktop app is quit and reopened for
   you as well: it reads its configuration at startup and will not reload changes made
   while running, so that step is what makes the new tier actually take effect.

Credentials and site data live in a local SQLite database under `~/.loongport/`, and are
sent only to the relay site you chose (as the Bearer token on its API calls — that is
what makes the account work). LoongPort has no account system and no server of its own,
so it never receives them.

## What it supports

| | Shipped | In progress |
|---|---|---|
| **Relay services** | sub2api | new-api |
| **AI CLIs** | codex | claude · gemini · grok |
| **Platforms** | macOS · Windows | Linux |

You can point it at your own site domain; a working one is preset by default. macOS and
Windows have the same feature set.

> One detail differs in the step that restarts the ChatGPT desktop app for you: **on
> macOS it asks the app to quit** — if ChatGPT has a conversation in progress it shows
> its own confirmation dialog and you can cancel (which aborts that switch); **on
> Windows the process is force-terminated**, with no dialog, so the app warns you
> before switching.

## Relationship to cc-switch

LoongPort was originally forked from
[cc-switch](https://github.com/farion1231/cc-switch) v3.19.1 and has since merged
upstream through v3.19.2; it is MIT-licensed too. A mature base saved a great deal of
duplicated work, and the icon is derived from theirs.

**They do different jobs.** cc-switch is a general multi-provider manager covering
every CLI and every provider, with a far wider feature set — proxy mode, MCP, Skills,
Prompts, Session Manager. LoongPort fully automates exactly one path: running Codex
cheaply through a relay service. If you want the broad tool, use cc-switch. The two
install side by side with separate data directories (`~/.cc-switch/` vs
`~/.loongport/`) and can run at the same time.

## Build from source

Requires Node.js 20+ and the Rust toolchain (1.85+).

```bash
git clone https://github.com/SailingLoong/LoongPort.git
cd LoongPort
pnpm install
pnpm tauri dev     # dev mode, hot reload
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

**Backend**: Tauri 2.8 · Rust (edition 2021, 1.85+) · serde · tokio · SQLite

```bash
cargo test --manifest-path src-tauri/Cargo.toml   # backend
pnpm vitest run                                   # frontend
pnpm tsc --noEmit                                 # type check
```

</details>

## Contributing

Issues and pull requests are welcome. Before touching the relay-account path, please
read [`LOONGPORT.md`](LOONGPORT.md) — it documents several behaviours that look wrong
until you know why they are that way (why `model_provider` must be `custom`, why
`requires_openai_auth` must be absent, why quitting ChatGPT goes by bundle id rather
than process name). Each one has a test pinning it, so changing them fails loudly
rather than silently.

## License

[MIT](LICENSE) — inherited from cc-switch, whose copyright notice is preserved.
