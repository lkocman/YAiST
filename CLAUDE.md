# YAiST: notes for coding agents and contributors

Read [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for building, running and
the settings; [docs/SYSTEM-INTERFACES.md](docs/SYSTEM-INTERFACES.md) for
the actions, `/commands`, completion and tool calls; [PLAN.md](PLAN.md)
for the design.

## Every change that users can notice updates the handbook

The handbook (`docs/handbook/`) is the user documentation and ships with
YAiST. It must describe what the code does now, in the same pull request:

- **Behaviour, requirements, limits, new states or labels in the clients:**
  update the prose in `docs/handbook/modules/<module>.adoc` (the chapter
  for the module; `welcome.adoc` for the window as a whole). Say what the
  user sees and what to do about it, in plain words.
- **New or changed actions** (id, title, description, parameters, shell
  recipe): run `yaist docs` (`cargo run -p yaist --bin yaist -- docs`, or
  `scripts/build-in-container.sh run -p yaist --bin yaist -- docs`). It
  rewrites the `// BEGIN GENERATED` blocks; never edit those by hand.
  `crates/yaist-modules/tests/handbook.rs` fails when they are stale.
- **New environment variables or launcher options:** the table in
  `docs/DEVELOPMENT.md`.
- **Screenshots** the handbook shows live in `docs/screenshots/`
  (`scripts/screenshots.sh`; made-up states with `YAIST_DEMO_SCENE`).

The "PR checks" workflow fails when code that users notice changes without
`docs/handbook/`; the `no-docs` label is for changes nobody can notice
(refactors, tests, CI).

## Building and checking

- No Rust toolchain needed: `scripts/build-in-container.sh <cargo args>`
  (Leap 16.0), e.g. `test --workspace`, `clippy --workspace --all-targets`.
- Formatting: check with `fmt --all --check` and fix only the files you
  changed; some committed files don't match the container's rustfmt, and
  `cargo fmt --all` would reformat them.
- Anything that needs root runs in a throwaway Tumbleweed or Leap podman
  container, not on the developer's machine.

## Rules the code relies on

- An action that changes the system is never `Risk::Read`.
- Arguments that reach commands or D-Bus are validated in
  `crates/yaist-core/src/validate.rs`.
- Client changes come with before/after screenshots in the pull request
  (the `no-screenshot` label when nothing visible changed).
- `main` is protected: work on a branch and open a pull request.
