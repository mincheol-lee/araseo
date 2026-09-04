# Araseo repository instructions

## Required verification

- After changing Rust production code, Slint UI code, build configuration, or
  runtime scripts, run `./scripts/verify` before reporting completion.
- Changes that affect the UI, Windows integration, dependencies, or a release
  executable must also pass `./scripts/verify --windows`.
- Do not treat compilation alone as verification when a headless behavioral
  test can cover the change. Add or update a Harness test for regressions.
- Keep tests pointed at the production modules under `src/`; do not copy their
  implementation into the Harness.
