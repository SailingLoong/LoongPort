# ZCode conflict layout regression

This harness imports the current checkout's real provider panel, Dialog and CSS.
Only the ZCode read/save API boundary returns synthetic data. It does not read
native configuration or prove native GUI integration.

## Browser runtime and CI hook

The existing `docs-ui.yml` workflow installs **playwright-core 1.63.0** outside the
product dependency tree and uses the runner's system Google Chrome with its
sandbox enabled. The ZCode matrix entry also builds this harness from the current
PR checkout and runs the three layout scenarios. Its screenshot source remains
pinned to the released UI; the regression source SHA is recorded separately as
`zcode-layout-source.txt`, alongside `zcode-layout-results.jsonl` in the review
artifact. No release screenshot pin or product dependency changes are needed.

The manual results below are independent of CI. A workflow definition alone does
not establish a passing GitHub Actions run; verify the run for the target source
SHA before reporting CI acceptance.

## Local reproduction

Use the repository's installed frontend dependencies and an isolated existing
Playwright runtime. To match the workflow, install `playwright-core@1.63.0` into a
separate temporary directory and set `PLAYWRIGHT_MODULE` to its `index.mjs`.
Choose an installed Chrome executable with `BROWSER_EXECUTABLE`.

```sh
node node_modules/vite/bin/vite.js --config tests/browser/vite.config.mjs
PLAYWRIGHT_MODULE=/path/to/playwright-core/index.mjs \
BROWSER_EXECUTABLE=/path/to/chrome \
node tests/browser/run-zcode-layout.mjs
```

To verify the same static build used by CI:

```sh
ZCODE_LAYOUT_OUTPUT=/tmp/zcode-layout \
node node_modules/vite/bin/vite.js build --config tests/browser/vite.config.mjs
python3 -m http.server 4314 --bind 127.0.0.1 --directory /tmp/zcode-layout
```

Run the browser command above in another terminal. `ENGINE=webkit` selects an
existing WebKit runtime; on macOS the harness uses Alt+Tab to traverse all controls.
The default Chromium engine uses Tab. WebKit is an additional local check, not a
job in this workflow.

## Recorded manual evidence

The original check used Playwright **1.63.0**, Node **26.9.0**, cached Chromium
headless shell revision **1234** and WebKit revision **2342**, explicitly selected
with `BROWSER_EXECUTABLE`. Those cached browsers are older than the package's
default revisions; the results establish that tested combination only. CI uses
its installed system Chrome rather than those cache revisions. The harness build
was tested with the already installed Vite **8.2.2** runtime.

All scenarios use a width of 1000 pixels:

| Height | Models | Resolution          |
| ------ | ------ | ------------------- |
| 650    | 80     | Keep my input       |
| 800    | 80     | Use external values |
| 800    | 1      | Keep my input       |

At the original `37fff407` source, focusing Keep after the 80-model comparison
placed it at y=2506–2542: outside both the viewport and Dialog, with no hit target.
The regression exited 1. With `min-h-0 overflow-y-auto` on the form, the same
regression exited 0. At height 650 the Dialog occupies y=32.5–617.5, Keep/Use
external occupy y=475–511 after keyboard focus, and Cancel occupies y=561–597.
The Chromium and WebKit runs both passed all three scenarios.

Assertions cover the Dialog's viewport bounds, keyboard reachability and hit
visibility of Keep/Use external/Cancel, password input masking and absence of the
synthetic key in comparison text, Keep retaining the entered key, Use external
clearing it, and explicit Save completing after resolution. JSON output records
the actual focused control bounds; it does not report a screenshot as native
acceptance.
