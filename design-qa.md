# Design QA

## Comparison target

- Source visual truth: `/mnt/d/dev/san/agy-switch/outputs/newapi-imagegen/concept-1.png`
- Intended viewport: 1440 × 1024 desktop
- Intended state: one selected local account, quick-switch targets visible, model quota populated

## Implementation evidence

- Browser-rendered implementation screenshot: not captured
- Primary interactions checked in browser: not checked
- Console errors checked: not checked

## Findings

- [Blocked] The selected visual has been translated into the source layout, but Product Design QA requires a real browser capture at the target viewport and a side-by-side visual comparison. No in-app Browser surface is available in this session, and the Product Design workflow requires the user's permission before using the Playwright MCP directly.

## Static evidence

- TypeScript passed with `npx tsc --noEmit`.
- An isolated WSL frontend copy completed `npm ci` and `npm run build` successfully.
- `cargo fmt --check` passed.
- The shared workspace's WSL `node_modules` cannot run Vite because its Linux Rollup optional package is absent; it was not reinstalled to avoid replacing the Windows-native dependency set.
- Rust test compilation remains environment-blocked: Linux needs GLib development headers, and Windows GNU cross-check needs the missing `x86_64-w64-mingw32-gcc` toolchain.

## Comparison history

No visual comparison iteration has run because browser capture is blocked before the first capture.

## Implementation checklist

- [x] Use the selected dark visual direction without copying invented navigation or data.
- [x] Keep the primary path to account selection and one-click target switching.
- [x] Move quota, import, and backup into supporting positions.
- [ ] Capture the rendered desktop view and compare it with the selected concept.
- [ ] Fix any P0/P1/P2 visual findings and rerun the comparison.

final result: blocked
