# Developing YAiST

Running from source, the command line, how changes get their privileges,
configuration, demos and screenshots. For what YAiST is, see the
[README](../README.md); for the design, [PLAN.md](../PLAN.md).

## Running from source

```sh
# 1. The local model (packaged llama-server, else rootless podman; first
#    start downloads ~2.5 GB). Without a usable discrete GPU it runs in CPU
#    mode, see docs/handbook/modules/assistant.adoc
scripts/llm-server.sh

# 2. Build (needs cargo; without it, builds in a Leap 16.0 container)
cargo build                      # or: scripts/build-in-container.sh build

# 3. Use it
target/debug/yaist-gui           # desktop
target/debug/yaist               # terminal (ctrl-q quits)
```

To get YAiST into the desktop's app menu with its icon (development build,
no root): `scripts/dev-install-desktop.sh`.

## Command line

```sh
yaist run /firewall overview             # any read action
yaist run /firewall open-port 8080 --yes # changes need --yes
yaist run /upgrade release-notes 16.1    # what's new in Leap 16.1 for this system
yaist ask "is ssh running?"              # one request through the agent; plans are never applied
yaist help [module]                      # all /commands
yaist tools                              # the tool definitions the model sees
```

## Privileges

Changes run with your privileges through polkit: systemd, firewalld and
timedated ask for authentication themselves; package, user and SELinux changes
use `pkexec`, and snapshots and Leap upgrades go through `yaist-helper`, a
small privileged program that re-validates its arguments. Plain pkexec asks
for the password every time; `scripts/dev-install-helper.sh` installs the
helper with a polkit policy that keeps the authorisation for a few minutes.
In the planned design all of this moves into a privileged `yaistd` daemon.

## Configuration

| Variable | Default |
|---|---|
| `YAIST_LLM_URL` | `http://127.0.0.1:8080` |
| `YAIST_LLM_MODEL` | `local`: the model name sent to the server |
| `YAIST_WELCOME_ART` | an SVG or PNG shown on the welcome screen instead of the drawn chameleon (e.g. the openSUSE logo) |
| `YAIST_THEME` | `light` starts the GUI in the light scheme |
| `YAIST_TEXT_SIZE` | starts the GUI at this text size (e.g. `150%`) instead of the saved one (`~/.config/yaist/text-size`) |
| `YAIST_FONT_DIR` | `/usr/share/yaist/fonts` |
| `YAIST_HELPER` | path to `yaist-helper`; default: the installed one, else next to the binary |
| `YAIST_RELEASE_NOTES` | a release-notes HTML page to read instead of the installed or online ones (testing) |
| `YAIST_END_OF_LIFE` | pretend the end of support is this date (`YYYY-MM-DD`), to see the END OF LIFE state |
| `YAIST_TOTAL_MEM_KIB` | pretend to have this much memory, to see the below-minimum notice (e.g. `3500000`) |
| `YAIST_OS_RELEASE` | pretend to be another flavour, e.g. `tests/os-release/leap-16.1-immutable` (development; check plans only) |
| `YAIST_MODEL` (llm-server.sh) | `unsloth/Qwen3-4B-Instruct-2507-GGUF:Q4_K_M` |
| `YAIST_MODEL_FILE` (llm-server.sh) | unset; a local GGUF to serve instead of `YAIST_MODEL` (yaist-offline's `yaist-llm.service` uses it) |
| `YAIST_LLM_ALIAS` (llm-server.sh) | `YAIST_MODEL`, or the file name: the model name the server reports |
| `YAIST_LLM_PORT` (llm-server.sh) | `8080` |
| `YAIST_LLM_IMAGE` (llm-server.sh) | `ghcr.io/ggml-org/llama.cpp:server-vulkan` |
| `YAIST_LLM_DEVICE` (llm-server.sh) | `auto`: the discrete GPU if usable (NVIDIA with its driver, AMD with own memory), else CPU mode, marked `@cpu` in the model alias; `gpu` or `cpu` forces one |

## Demos

States that are hard to reach on purpose, such as a change that failed or an
upgrade half-way, come as made-up scenes. Type `/demo` in the prompt for the
list, `/demo <scene>` to show one. They work in the GUI and the TUI, and
stay out of `/help`. The cards are built from the real actions, with only
their outcome invented; their buttons do what they always do (Retry proposes
the real change, report links open the browser).

| Scene | Shows |
|---|---|
| `report-distro` | a package's `%post` script fails: "Report a Tumbleweed bug" suggested |
| `report-yaist` | YAiST misreads zypper: "Report a YAiST issue" suggested |
| `upgrade-incomplete` | an upgrade that stopped part-way: continue it or roll back |
| `upgrade-running` | an upgrade under way, offering what's new meanwhile ("Tell me" really reads the notes) |
| `whats-new` | what that tells: the release-notes entries for this system and a summary |

The scenes are in [crates/yaist-ui/src/demo.rs](../crates/yaist-ui/src/demo.rs).

## Screenshots

`scripts/screenshots.sh` regenerates `docs/screenshots`: the GUI runs in a
throwaway Tumbleweed container and draws on the desktop's Wayland session.
Arguments pick the shots (`scripts/screenshots.sh whats-new`); none takes
them all. It needs a built `target/debug/yaist-gui`, Wayland and ImageMagick.

The GUI takes one shot and quits when told:

| Variable | Does |
|---|---|
| `YAIST_SCREENSHOT` | saves a frame to this `.ppm` file, then quits |
| `YAIST_SCREENSHOT_DELAY` | seconds before the shot (default 4) |
| `YAIST_MODULE` | selects module N at start |
| `YAIST_DEMO` | submits this request at start, as if typed |
| `YAIST_DEMO_CONFIRM` | `1` confirms the plan the request raises |
| `YAIST_DEMO_CLICK` | `x,y[,clicks[,seconds]]` clicks there (2 for a double-click), after 2 seconds or as given |
| `YAIST_DEMO_SCENE` | shows a demo scene at start, without the `/demo` line |
| `YAIST_DEMO_TYPE` | types this into the conversation input at start, without sending it; each tab presses Tab (e.g. `$'/demo \t\t'` shows the completion) |

## The openSUSE Handbook

`docs/handbook` is a book (AsciiDoc, CC-BY-SA-4.0) with one chapter per
module, `modules/<module>.adoc`, included from `opensuse-handbook.adoc`.
Chapters are written by hand, except blocks between
`// BEGIN GENERATED <key>` and `// END GENERATED`: one per action (what it
does, whether it asks first, its options, the terminal commands on each
flavour) and one per module (the at-a-glance table). They are rendered from
the `ActionSpec`s and `reg.shell` equivalents, so after changing an action:

```sh
scripts/build-in-container.sh run -p yaist --bin yaist -- docs
```

This rewrites the blocks, adds a stub section (marked `TODO(handbook)`) for
a new action and a stub chapter for a new module, which you then add to the
book. `cargo test` fails while the handbook is out of date.
`scripts/build-handbook.sh [pdf] [epub] [html]` builds the book into
`target/handbook` (in a container; `scripts/handbook-build.sh` is the build
itself, shared with CI). The Handbook workflow builds it for every push and
pull request, publishes it on GitHub Pages from main and attaches the PDF
and EPUB to published releases.

## Layout

```
crates/yaist-core      action model: specs, validation, /command syntax, registry
crates/yaist-modules   system, users, services, firewall, selinux, packages, repos, …
crates/yaist-agent     llama-server client and tool-calling agent
crates/yaist-ui        shared client model (state, plans, forms, palette, demos)
apps/yaist             TUI + CLI
apps/yaist-gui         egui desktop app
docs/handbook          the openSUSE Handbook, one chapter per module
```
