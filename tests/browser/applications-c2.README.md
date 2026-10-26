# Applications C2: current-checkout browser evidence

This isolated fixture imports the **real `src/App.tsx`, ordinary providers, i18n,
and `src/index.css`**. It is not a hand-drawn reproduction of the approved image.

All native DTOs/receipts are synthetic, browser-memory-only data installed with
Tauri's official `mockIPC` / `mockWindows`. Unknown IPC rejects, including unknown
`plugin:event|…` commands (the official event mock is otherwise permissive).
Only explicit enumerated startup reads and fixture-local actions are accepted.
No real accounts, credentials, configuration writes, installation or native
forwarding are exercised. The empty unrelated account-manager inventories are
adjunct fixture DTOs, not evidence that a shared-group member is unavailable.

## Scenario and boundaries

- Saved Codex target: manual `MiniMax A / model-a`.
- Global `日常编码` group order: `Qoder C / fixture-qoder-model`,
  `MiniMax A / model-a`, `WorkBuddy B / model-b`.
- The shared DTO has no runtime member eligibility, skip reasons or request trace.
  All three members remain **待核验**; the third is not fabricated as unavailable.
- Software attachment comes from the original `modeState`. Saved configuration,
  local draft and actual-request-unverified state stay separate.
- `normal`: changing selection only creates a draft; native prepare yields the
  review receipt; explicit confirmation applies once in fixture memory.
- `unknown`: confirmation returns unknown, preserves the prior saved binding,
  and explicit inspection never replays the apply or invents a recovery journal.
- `pending`: only the original app-local pending configuration receipt can be
  recovered; this fixture discards that configuration operation, never retries
  a model request.

## Existing CI entry, unchanged trust boundary

`docs-ui.yml`'s existing zcode current-checkout build/runner step also builds and
runs this fixture. The stable/zcode released-source pins, existing runners,
Playwright 1.63.0 install, sandboxed Google Chrome, `contents: read` permission
and seven-day artifact retention are unchanged. No new dependency or browser
installation is needed.

```sh
pnpm exec vitest run --config tests/browser/applications-c2.vitest.config.mts
pnpm exec vite build --config tests/browser/vite.config.mjs
# In the existing CI loopback renderer (127.0.0.1:4314):
APPLICATIONS_C2_SOURCE_SHA="$(git rev-parse HEAD)" \
  node tests/browser/run-applications-c2.mjs
```

The DOM smoke uses an independent config, one worker, the actual App and official
Tauri mocks—not the broader existing MSW IPC mocks. Its `.dom.tsx` suffix avoids
accidental collection by the default MSW-backed unit config; the isolated config
includes it explicitly. It blocks fetch, XHR, WebSocket, EventSource and beacon
transports before App import and asserts zero attempted transport calls throughout
the real-App tests. It is **not browser evidence**.

The browser runner captures 1440×1040 and 760×1040 states, including original
48×44 app tabs, 40×44 plus and 20×20 hide buttons, add/hide semantics, local draft,
review, configuration saved only, unknown inspection and original-operation
recovery. It checks horizontal and vertical content overflow, text ranges against clipping
ancestors, radio/control reachability, and compact desktop member rows (≤60 px
for these short synthetic names only). It rejects
renderer errors/unimplemented IPC, blocks outbound requests/WebSockets and
allows only GET renderer resources from the existing loopback static origin.
Top and footer captures are separate when the real main content scrolls.

Artifacts include exact checkout SHA, relevant source hashes, screenshot hashes,
fixture declaration and approved image SHA-256
`05d8b77c3a164e69eeb03ab7f8ea03691d90bda80988a1143c81b2d6f5b41544`.
Failure artifacts retain the actual error and intercepted commands/requests.

Passing browser checks prove rendering/interaction only. Human comparison with
the approved image, native/WebView operation and live-account/model-request
acceptance are separate and cannot be inferred from these fixtures.
