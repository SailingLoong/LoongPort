# Product-guide UI capture

These are documentation fixtures, not an application demo mode. They import the
released application's real React components, styles and translations; only the
Tauri IPC boundary is mocked using Tauri's official `mockIPC` API. All identities,
service domains, balances and activity are synthetic. No service credentials or
existing user data are read. Browser requests outside the local renderer are
blocked. A screenshot proves UI rendering only, not native integration, login,
configuration writes, billing, routing or an API request.

The workflow pins stable v6.26.2 and the ZCode-only v6.26.3-beta.2 appendix to
their exact release commits and records each source/version in its artifact. Do not update that pin or label without reviewing the guide. ZCode has a separate beta job and must never appear in a stable screenshot.

Run the workflow in a draft pull request. It uses Chromium with its sandbox on,
CJK fonts, and a short-lived artifact. Review actual pixels and run OCR before
copying accepted images into `docs/guide/images/`; raw artifacts are not published
automatically. Numbered callouts are added to clean copies after review, not to
production UI code. Keep the original capture and SHA-256 in the capture record.

For local component development, install the repository's pinned dependencies,
set `LOONGPORT_UI_SOURCE` to a clean checkout of the release commit, then run:

```sh
pnpm exec vite build --config scripts/docs-ui/vite.config.mts
```

The fixture never sets a protection password or bypasses the real vault. Native
first-launch testing is a separate check in an isolated profile.
