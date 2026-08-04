<div align="center">

<img src="assets/branding/loongport-icon-master.png" alt="" width="96" height="96">

# LoongPort

### Codex at 5% of official cost, Claude at 20%, no extra network setup

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

LoongPort itself is free. It never handles your payment and takes nothing out of your
balance — it only writes the config correctly. You top up with the relay provider.

> **One relationship, stated up front**: registration links carry our referral code
> (a compile-time table in `src-tauri/src/operator/aff.rs` — visible in the source),
> and we may earn a rebate from the relay site as a result. **This does not affect your
> price and nothing is deducted from your balance** — but you deserve to know it exists.

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
| **Windows** | Windows 10 or later |
| **macOS** | macOS 12 (Monterey) or later |

Download from the [Releases](../../releases) page. Every release is built automatically
by GitHub Actions, for both Windows and macOS:

| Platform | File | Notes |
|---|---|---|
| **Windows** | `LoongPort-v{version}-Windows-Portable.zip` | **Portable, recommended** — unzip to get a single `LoongPort.exe` and run it; no Windows Installer involved |
| | `LoongPort-v{version}-Windows.msi` | Installer (adds a Start menu entry) |
| **macOS** | `LoongPort-v{version}-macOS.dmg` | — |

On ARM64 Windows machines (Snapdragon laptops and the like), use the two files with
`-arm64` in the name — again one installer and one portable build.

> **On Windows, prefer the portable build.** It unzips to a single exe (the WebView2 loader is
> statically linked, so no extra DLLs are needed next to it) — put it anywhere and
> double-click. Security software sometimes blocks the Windows Installer from
> **backing up old files** (reported as `could not set file security for file
> '...\Config.Msi\xxxxxxx.rbf'  Error: 5`); the portable build has no install step, so
> there is nothing to block.
>
> It does need the WebView2 runtime, which Windows 10 and later generally ship with.

> **macOS blocks the first launch.** The macOS build is not code-signed or notarized by
> Apple, so Gatekeeper reports the app as "damaged" — it is not. Run this once in
> Terminal; **once is enough**:
>
> ```bash
> xattr -dr com.apple.quarantine /Applications/LoongPort.app
> ```
>
> Signing and notarization need an Apple Developer account ($99/year) and will come in a
> later release — it only affects this one installation step, and once installed the app
> works exactly as it does on Windows.

> **Already on the installer and an upgrade fails with "could not set file security".**
> On a few machines running security software (Tencent PC Manager, for one), installing
> *over* an older version fails with `could not set file security for file
> '...\Config.Msi\xxxxxxx.rbf'  Error: 5` — that is the security software blocking the
> installer from **backing up the old files**, not a broken package. Uninstall the old
> version first (Settings → Apps), then run the new installer: a fresh install has
> nothing to back up, so nothing gets blocked. **Your account and settings are kept** —
> they live in your user folder, untouched by uninstall, so you stay signed in.
>
> First-time installs and the portable build are both unaffected; this only happens when
> overwriting an older version.

## How it works

1. **Enter a domain** — your relay provider's site. Leave it blank to use the default.
2. **Sign in** — the provider's real login page loads in a window; register or sign in
   right there. LoongPort receives the resulting credentials and never sees your password.
3. **Keys provisioned** — one per available tier. Existing keys with matching names are
   reused before new ones get created, so hitting refresh never litters your account.
4. **Pick a CLI and tier** — Codex uses OpenAI tiers, Claude uses Anthropic tiers. One
   click writes the matching config (`~/.codex/config.toml`, or Claude's settings).
5. **Keep working** — Codex or Claude Code just works. **When switching a Codex tier**,
   the ChatGPT desktop app is quit and reopened for you as well: it reads its
   configuration at startup and will not reload changes made while running, so that step
   is what makes the new tier actually take effect. Claude tier switches do not involve
   it and leave your open windows alone.

Credentials and site data live in a local SQLite database under `~/.loongport/`, and are
sent only to the relay site you chose (as the Bearer token on its API calls — that is
what makes the account work). LoongPort has no account system and no server of its own,
so it never receives them.

## What it supports

| | Shipped | In progress |
|---|---|---|
| **Relay services** | sub2api | new-api |
| **AI CLIs** | codex · claude | gemini · grok |
| **Platforms** | macOS · Windows | Linux |

You can point it at your own site domain; a working one is preset by default. macOS and
Windows have the same feature set.

> One detail differs, and only when switching a **Codex** tier (that is the step which
> restarts the ChatGPT desktop app for you; Claude switches leave it alone): **on macOS
> it asks the app to quit** — if ChatGPT has a conversation in progress it shows its own
> confirmation dialog and you can cancel (which aborts that switch); **on Windows the
> process is force-terminated**, with no dialog, so the app warns you before switching.

## Acknowledgements and upstream projects

LoongPort stands on two other people's projects. Here is where each one fits.

### cc-switch — the base this is built on

LoongPort was originally forked from
[cc-switch](https://github.com/farion1231/cc-switch) (by
[@farion1231](https://github.com/farion1231)) v3.19.1 and has since merged upstream
through v3.19.2; it is MIT-licensed too, their copyright notice is kept in
[LICENSE](LICENSE), and the icon is derived from theirs. **Most of the code in this
repository is theirs** — a mature base saved a great deal of duplicated work, and this
project would not exist without it.

**They do different jobs.** cc-switch is a general multi-provider manager covering
every CLI and every provider, with a far wider feature set — proxy mode, MCP, Skills,
Prompts, Session Manager. LoongPort fully automates exactly one path: running an AI CLI
cheaply through a relay service. **If you want the broad tool, go use cc-switch** — it
is still actively maintained. The two install side by side with separate data
directories (`~/.cc-switch/` vs `~/.loongport/`) and can run at the same time.

### sub2api — the relay backend it talks to

Most relay sites run [sub2api](https://github.com/Wei-Shaw/sub2api) (LGPL-3.0).
LoongPort is a **plain HTTP client** of it — it does not link against, bundle, or reuse
any of its code; it only calls the interfaces sub2api documents (endpoints, fields, auth
scheme). Those interfaces being clear is what makes "provision keys and read tiers
automatically" possible at all.

LoongPort is not an official sub2api client and is not affiliated with its author.
Please report problems you hit with LoongPort here, not upstream.

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

[MIT](LICENSE), same as upstream cc-switch. [LICENSE](LICENSE) carries two copyright
lines — the upstream author's, kept verbatim as MIT requires, and this project's own for
the changes made here.
