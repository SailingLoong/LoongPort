# Windows installation directory

The interactive NSIS Setup lets users choose an installation folder. The default
is `%LOCALAPPDATA%\LoongPort`; choose a folder writable by the current user.
Later NSIS installations restore the saved folder as their initial selection.
Application updates reuse that folder without asking for it again.

## Ownership and source evidence

Tauri owns directory selection and persistence. Keep the stock template: no
custom template or path-setting hook is needed. `installMode` controls installation
scope, not whether users can choose a folder. The omitted value means
`currentUser`, with registry metadata under HKCU; see the
[official configuration reference](https://v2.tauri.app/reference/config/#nsisinstallermode)
and [Windows installer guide](https://v2.tauri.app/distribute/windows-installer/).

Checked on 2026-09-13 against the locked `@tauri-apps/cli` 2.11.4. The complete
32,007-byte [official template at that CLI tag](https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.11.4/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi)
was found byte-for-byte in the installed CLI native binary. This verifies the
bundled template, rather than assuming the current upstream branch matches it.

- Lines 388–389 insert `MUI_PAGE_DIRECTORY`, with `SkipIfPassive` as its pre-hook.
  Lines 907–909 skip it only when `$PassiveMode = 1`. NSIS silent mode also omits UI.
- Lines 499–518 set the default only while `$INSTDIR` is the placeholder, then
  call `RestorePreviousInstallLocation`. An explicit NSIS `/D=` argument bypasses
  that default/restore block.
- Lines 681–682 persist `$INSTDIR` to the default value of `${MANUPRODUCTKEY}`.
  Lines 897–901 read that value and restore it when nonempty. With this project's
  product name, publisher and current-user scope, the key is
  `HKCU\Software\SailingLoong\LoongPort`.
- `tauri.conf.json` has no NSIS template or installation-scope override;
  `tauri.windows.conf.json` has no bundler override. `installer-hooks.nsh` only
  reads `$INSTDIR` to check the target executable. Neither MSI migration nor the
  file-lock wait assigns it or replaces the directory page.

The existing `tests/lib/windowsInstallerContract.test.ts` guards these local
configuration assumptions and direct hook path/page overrides. It does not execute
NSIS or validate every possible hook instruction. Recheck the bundled template
when upgrading the CLI; do not copy it into this repository.

## Updates and release evidence

The locked `tauri-plugin-updater` 2.10.1 defaults to **passive**, not fully silent:
`src/config.rs` maps passive to `/P /R`, quiet to `/S /R`; `src/updater.rs` adds
`/UPDATE` and launch arguments. LoongPort supplies no installation-mode or
installer-argument override. Passive shows progress but skips directory selection;
`/UPDATE` avoids the normal NSIS uninstall step. The saved directory is restored
in `.onInit` before either mode installs files. A plain `/S` invocation and an
application update are therefore different commands.

Read-only inspection of [v6.24.0-beta.1](https://github.com/SailingLoong/LoongPort/releases/tag/v6.24.0-beta.1)
found x64 and arm64 `Setup.exe` assets, their `.sig` files, and separate Portable
ZIPs. Its `latest.json` maps both Windows architectures to the corresponding
Setup executable with a nonempty signature. Its lockfile uses CLI 2.11.4; its
hooks and Windows overlay match the inspected source. Asset presence and manifest
inspection are not execution or cryptographic verification of the installers.

`release.yml` builds NSIS and collects Setup executables for both Windows
architectures. `windows-build.yml` also builds NSIS, disabling updater artifacts
for unsigned test builds. Neither workflow runs an installer or exercises a
silent upgrade; successful packaging alone does not prove directory retention.

## Windows verification still required

The inspection and contract tests run on macOS. No Windows installer UI, registry
writes, permissions, or upgrade execution were tested here. On Windows, verify:

1. Run Setup interactively on a clean current-user installation. Select a writable
   nondefault folder containing a space; confirm the executable, shortcuts and
   saved registry directory point there.
2. Run a newer Setup interactively. Confirm the directory page starts at the
   saved folder and completing the upgrade retains it.
3. Upgrade through the application. Confirm progress appears without a directory
   prompt, the executable updates in place, and the app restarts from that folder.
4. Separately exercise `/S /UPDATE` and confirm the exit result, installed version
   and retained folder. Check both released architectures on Windows.

These retention claims cover NSIS-to-NSIS upgrades with the same user and registry
identity. The legacy MSI migration hook does not import the old MSI directory;
do not treat its uninstall support as evidence of MSI custom-path retention.
