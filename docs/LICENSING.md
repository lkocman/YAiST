# Licensing

YAiST is licensed under **Apache-2.0**. This page is for packagers and
legal reviewers (Cavil, the openSUSE packaging skill): what is in the source,
what ends up in the binaries, and what is only downloaded at runtime.

## Where the facts live

| File | What it says |
|---|---|
| `LICENSE` | The project licence (Apache-2.0), at the repository root |
| `LICENSES/` | Every licence text used anywhere in the tree (REUSE layout) |
| `REUSE.toml` | Per-file copyright and SPDX identifier; `reuse lint` checks it |
| `NOTICE` | Attribution that redistributors must keep |
| `Cargo.toml` (workspace) | `license = "Apache-2.0"`, inherited by every crate |
| `deny.toml` | Allowed crate licences; `cargo deny check licenses` enforces it |
| `scripts/license-report.sh` | Linked crates per binary and the suggested `License:` tag |

## The source tree

| Path | Licence | Notes |
|---|---|---|
| everything not listed below | Apache-2.0 | YAiST's own work |
| `apps/yaist-gui/src/theme.rs`, `widgets.rs` | Apache-2.0 | adapted from the ArtCraft launcher (MIT OR Apache-2.0, used under Apache-2.0); both copyright holders in `REUSE.toml` |
| `assets/fonts/*.ttf` | OFL-1.1 | Inter, Archivo, Geist Mono; texts in `assets/fonts/OFL-*.txt` |
| `assets/art/sinking.svg` | **GPL-3.0-or-later** | the welcome screen's pirate Geeko, from openSUSE-welcome; a separate data file loaded at run time (installed to `/usr/share/yaist/art/`), never compiled into the binaries |
| `assets/icons/` | Apache-2.0 | the YAiST icon, generated with ChatGPT by Lubos Kocman (OpenAI's terms assign the output to the user; purely AI-generated images may not be copyrightable, whatever rights exist are under Apache-2.0) |

**Package impact of the welcome artwork:** the binaries stay Apache-2.0 (plus
their crates), but a package that ships `assets/art/sinking.svg` contains a
GPL-3.0-or-later file: add `GPL-3.0-or-later` to that package's `License:` or
ship the artwork in its own subpackage. Without the file, the welcome screen
falls back to the chameleon drawn in code. A permissive grant from the
openSUSE-welcome artwork authors would remove this.

No ArtCraft trademarks (name, logo, icons) are used. The look borrows only the
layout and widget style, in the openSUSE colour palette.

## The binaries

Rust links its dependencies statically, so each binary's licence is YAiST's
plus whatever its crates add. Output of `scripts/license-report.sh` at the
time of writing (x86_64 Linux target, normal dependencies only):

| Binary | Crates linked | Suggested `License:` |
|---|---|---|
| `yaist` (TUI and CLI) | 161 | `Apache-2.0 AND BSD-3-Clause AND ISC AND MIT AND Unicode-3.0 AND Zlib` |
| `yaist-gui` | 285 | `Apache-2.0 AND BSD-2-Clause AND BSD-3-Clause AND ISC AND MIT AND OFL-1.1 AND Unicode-3.0 AND Zlib` |

- **No copyleft is forced.** Two crates offer a copyleft licence as one `OR`
  choice: `r-efi` (`MIT OR Apache-2.0 OR LGPL-2.1-or-later`) and `self_cell`
  (`Apache-2.0 OR GPL-2.0-only`). Both are used under Apache-2.0.
- **MIT** appears because ~40 crates are MIT-only; crates offering
  `MIT OR Apache-2.0` are used under Apache-2.0.
- **OFL-1.1** in `yaist-gui` comes from the bundled fonts. Building with
  `--no-default-features` (no `bundled-fonts`) drops the embedded fonts; the
  binary then reads them from `/usr/share/yaist/fonts` (or `$YAIST_FONT_DIR`).
  Inter is packaged in openSUSE (`inter-fonts`); Archivo and Geist Mono are
  not, which is why they are bundled by default.
- TLS (for get.opensuse.org) is rustls with the **system** CA store
  (`/etc/ssl`), not a bundled certificate list.
- egui's default fonts (which carry the Ubuntu Font Licence) are deliberately
  disabled.

**Re-run `scripts/license-report.sh` after every dependency update.** A
re-vendor can add licences the previous one didn't have, so an existing
`License:` tag is not evidence for the next version. Inside an rpmbuild with
the vendored tree, pass `--offline`.

### Suggested spec text

```spec
# Legal-Review-Notice: Rust crates are linked statically. Derived with
# scripts/license-report.sh (cargo tree -e normal -p yaist / -p yaist-gui):
# no copyleft without a permissive alternative (r-efi and self_cell are
# used under Apache-2.0); OFL-1.1 covers the fonts embedded in yaist-gui.
License:        Apache-2.0 AND BSD-3-Clause AND ISC AND MIT AND Unicode-3.0 AND Zlib
...
%package gui
License:        Apache-2.0 AND BSD-2-Clause AND BSD-3-Clause AND ISC AND MIT AND OFL-1.1 AND Unicode-3.0 AND Zlib
```

## Compatibility with what YAiST talks to

- **Separate programs** (zypper, systemctl tools, useradd, pkexec) and **D-Bus
  services** (systemd, firewalld, polkit) are not linked; their licences do
  not affect YAiST's.
- **A future libzypp binding** (libzypp is GPL-2.0-or-later) would make the
  combined binary distributable under GPL-3.0 terms, since Apache-2.0 is
  compatible with GPLv3 but not GPLv2. Decide that consciously when the
  binding lands, and record it in the spec's `License:`.
- **GPL-2.0-only code cannot be linked** into an Apache-2.0 program; keep it
  out of the dependency tree (`deny.toml` already rejects it).

## Not part of YAiST: distribution logos

The header wordmark and the welcome screen's logo are read at runtime from
`/usr/share/pixmaps/distribution-logos/`, which every openSUSE flavour installs
(`distribution-logos-openSUSE-*`, CC-BY-SA-4.0, openSUSE trademarks). YAiST
ships no logo files; without the package it shows the name as text. A package
can `Recommends:` the logos. The chameleon on the welcome screen is YAiST's own
drawing, made in code.

## Not part of YAiST: the model and its server

`scripts/llm-server.sh` downloads these on the user's machine. They are not in
the source tarball, the binaries or the package.

| Component | Licence | Source |
|---|---|---|
| llama.cpp server | MIT | `ghcr.io/ggml-org/llama.cpp:server-vulkan` (Ubuntu-based image with its own package licences) |
| Qwen3-4B-Instruct-2507 weights | Apache-2.0 | Qwen team; GGUF conversion by unsloth on Hugging Face |

YAiST talks to any OpenAI-compatible server, so the model is a configuration
choice. Model choices must stay OSI-licensed (Apache-2.0 / MIT); Llama and
Gemma licences do not qualify.

For a distribution, the plan is to replace the download with packages built in
OBS: llama.cpp as an RPM or an openSUSE-based container, and the weights as a
`noarch` data package (e.g. `yaist-model-qwen3-4b`, Apache-2.0) so they get
the same legal review as everything else.
