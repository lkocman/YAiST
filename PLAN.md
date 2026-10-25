# YAiST — plan

A local, AI-assisted successor to YaST for openSUSE. Users manage the system by
talking to an agent, or through classic panels/forms. Native TUI and GUI, no web
app. The LLM runs locally and must be an open-source (OSI-licensed) model.

**Targets: every openSUSE distribution** — Leap, Tumbleweed, Slowroll and the
immutable variants (MicroOS, Aeon, Kalpa, Leap Micro). Actions detect the
flavour (`/etc/os-release`, read-only root + `transactional-update`) and adapt:
`zypper up` vs `zypper dup` vs `transactional-update`, OBS repository paths.

## Prototype status (2026-10-23)

Working: Rust workspace, 7 modules / 36 actions, TUI + egui GUI on one shared
model, `/commands`, plan cards, generated forms, local agent, CLI (`yaist run`,
`yaist ask`). Measured on a Ryzen 7840U laptop (Radeon 780M iGPU via Vulkan,
Qwen3-4B-Instruct-2507 Q4_K_M in llama.cpp):

- request → plan card or answer: **1.0–4 s** typical; ~28 tokens/s generation,
  ~330–390 tokens/s prompt processing
- the system prompt + 36 tool schemas are ~3.8k tokens: **~10 s if not
  cached**, so the client warms the server's prompt cache at startup
- a small model needs a tight prompt: forbidding narration and text
  confirmations, plus a few examples, cut most requests from 2–3 LLM rounds
  to one

Learned:

- openSUSE's polkit policy wants admin auth even to *read* firewalld
  (`config.info` is `auth_admin_keep`); reading users' journals works via
  `wheel`. Another reason reads belong in `yaistd`.
- zypper's XML covers search/repos/updates but not `info`; zypper reports
  "nothing found"/"repos skipped" via exit codes 104/106.
- llama-server supports Unix sockets (`--host /path.sock`), so the planned
  socket-only setup works.

Not yet: `yaistd` (privileged ops go through pkexec / service polkit),
snapper snapshots, migration module, router for tool subsets, eval set.

## Core principle: one action set, every UI

Everything the user can do in YAiST is a typed **action** in a single registry.
Every action is reachable from the **input prompt**, and every panel/button is
just another way to fill in and invoke the same action.

```
┌──────────┐  ┌──────────┐  ┌──────────┐
│ GUI      │  │ TUI      │  │ CLI      │   unprivileged clients
│ prompt + │  │ prompt + │  │ yaist …  │
│ panels   │  │ forms    │  │          │
└────┬─────┘  └────┬─────┘  └────┬─────┘
     └──────┬──────┴─────────────┘
       ┌────▼─────┐        ┌───────────────┐
       │ agent /  │◄──────►│ llama-server  │  local LLM
       │ resolver │        └───────────────┘
       └────┬─────┘
            │ D-Bus (system bus) + polkit
       ┌────▼──────────────────────────────┐
       │ yaistd — privileged daemon         │
       │ action registry (typed, versioned, │
       │ risk-levelled, dry-run capable)    │
       └──┬───────┬───────┬───────┬────────┘
       systemd firewalld SELinux zypper/libzypp, snapper, migration tool
```

Each action definition (e.g. `firewall.open_port {zone, port, proto}`) yields:

- a JSON schema exposed to the LLM as a tool,
- an auto-generated form/panel in GUI and TUI,
- a CLI subcommand and a prompt command.

## The input prompt

One input box (GUI and TUI) that accepts three kinds of input:

| Input | Example | Path | Latency |
|---|---|---|---|
| Direct command | `/firewall open 8080/tcp` | parsed straight to an action, no LLM | instant |
| Natural language | "let my nephew log in but without sudo" | LLM → action call(s) | target < 5 s |
| Question | "why is nginx failing?" | LLM + read-only actions → answer | target < 5 s |

- Autocomplete over action names and arguments (doubles as a command palette).
- Whatever the source, the resolved action is shown as a **plan card** before
  anything changes: action, arguments, exact effect / diff, risk level.
- The user confirms, edits the arguments in the generated form, or cancels.
- Panels do the reverse: clicking "Open port" in the Firewall panel shows the
  equivalent `/firewall open …` command, so users learn the prompt syntax.

## Client layout (shared by GUI and TUI)

GUI and TUI use the same model and layout; the TUI is the same app rendered in
a terminal, not a reduced chat-only mode. Both are built on one client-side
view model (modules, panels, plan cards, conversation), so a feature added to
one appears in the other.

```
┌ YAiST ─────────────────────────────────────────────────────────────────┐
│ Modules        │ Firewall · zone: public                               │
│ ▸ System       │ ┌ Open ports ────────────┐ ┌ Services ─────────────┐ │
│   Users        │ │ 22/tcp   ssh       [x] │ │ ssh            [x]    │ │
│   Services     │ │ 8080/tcp           [x] │ │ http           [ ]    │ │
│ ▸ Firewall     │ │ [+ Open port]          │ │ [+ Allow service]     │ │
│   SELinux      │ └────────────────────────┘ └───────────────────────┘ │
│   Packages     ├───────────────────────────────────────────────────────┤
│   Repositories │ you › open http for the web server                    │
│   Migration    │ YAiST › Plan: firewall.allow_service                  │
│                │         zone=public service=http   risk: change       │
│                │         [Confirm]  [Edit]  [Cancel]                   │
│                ├───────────────────────────────────────────────────────┤
│                │ › /firewall open 443/tcp▏                             │
└────────────────┴───────────────────────────────────────────────────────┘
```

- **Panel area**: the actionable UI for the selected module (lists, toggles,
  forms) — the classic YaST experience, keyboard-driven in the TUI.
- **Conversation shell**: chat history, plan cards, results.
- **Input prompt**: `/commands` and natural language.
- The two halves stay in sync: an action run from the conversation updates
  the panel (and navigates to it); an action run from a panel is echoed into
  the conversation as its equivalent command.
- **Same scope everywhere**: the set of actions the LLM can call as tools is
  exactly the set the panels and `/commands` expose — no AI-only or UI-only
  capabilities.
- TUI layout collapses gracefully on narrow terminals (panel and conversation
  as switchable tabs).

## Safety

- The LLM never gets a shell; it can only call registered actions.
- Risk levels: `read` (auto-run), `change` (confirm), `dangerous` (confirm +
  explicit warning, e.g. distribution migration, removing users).
- Snapper pre/post snapshot around every change; rollback is an action too.
- polkit authorisation in the daemon; audit log of every executed action.
- Tool output (package descriptions, logs, repo metadata) is treated as
  untrusted data (prompt-injection).

## Security model

### Threats, in order of real-world risk

1. **Local privilege escalation through yaistd's D-Bus API.** Any process
   running as the user can call it — not just our clients. This is where
   PackageKit/udisks/polkit-class bugs come from.
2. **Prompt injection.** Journal lines, package descriptions, repo metadata,
   unit descriptions can be attacker-controlled (a logged HTTP request is
   enough) and steer the model.
3. **Model mistakes.** Wrong action or wrong arguments.
4. **LLM runtime compromise.** llama.cpp is a large C++ codebase parsing
   model files; GGUF parser CVEs exist.
5. **Model supply chain.** Tampered model weights.

### Privilege layout

| Component | Runs as | Isolation |
|---|---|---|
| GUI / TUI / CLI clients | user | none needed — **untrusted** from yaistd's view |
| yaist-agent | user | no network access except the LLM socket |
| llama-server | dedicated unprivileged user | sandboxed: no network, read-only model dir, only `/dev/dri` |
| yaistd | root | systemd hardening where compatible; SELinux policy module |
| zypper / migration helpers | root, spawned per job | separate process, own cgroup, killable |

### Containers: where they help and where they don't

- **yaistd in a privileged container: no.** A privileged container is root on
  the host with extra steps. yaistd has to touch the host rpmdb, `/etc`,
  systemd, firewalld, snapper, bootloader — so it would need everything
  mounted back in, and zypper would then manage the container instead of the
  host unless chrooted into `/host`. Packaging churn, zero isolation gain.
- **llama-server sandboxed: yes.** It is the largest, least-trusted code and
  needs almost nothing: a model file, a GPU device, a socket. Use systemd
  sandboxing (`DynamicUser=`, `PrivateNetwork=`, `ProtectSystem=strict`,
  `ProtectHome=`, `DeviceAllow=/dev/dri`, `SystemCallFilter=`,
  `MemoryDenyWriteExecute=` if the GPU backend allows). A rootless container
  (e.g. via `ramalama`, packaged in openSUSE) is an equivalent alternative;
  systemd units are simpler and need no container runtime.
- **Shipping yaistd as a container for immutable variants:** decide later;
  a package plus `transactional-update` is the natural fit.

### Rules for yaistd

- **polkit is the security gate; the client's Confirm button is only UX.**
  Every mutating action has its own polkit action ID
  (`org.opensuse.yaist.firewall.change`, …), checked against the D-Bus caller
  identified by its bus name (system-bus-name subject), never by PID.
- Default policy: `read` actions allowed for active local sessions; `change`
  → `auth_admin_keep`; `dangerous` → `auth_admin` every time. Admins can
  tune via polkit rules.
- Strictly typed, validated arguments (usernames, unit names, ports, repo
  URLs, SELinux booleans) — validated in yaistd, not the client.
- No shell, ever: subprocesses via argv, fixed binary paths, clean env.
- Prefer delegating to existing services over reimplementing (firewalld,
  systemd, hostnamed, snapperd) so the root code in yaistd stays thin.
- Long-running jobs (zypper, migration) run in separate helper processes;
  yaistd only orchestrates.
- Audit every executed action to the journal: caller UID, action, arguments,
  result, snapshot IDs.

### Prompt-injection containment

- Tool output is wrapped and marked as data in the prompt, and trimmed.
- The model can only *propose* changes; the plan card shown to the user is
  rendered by trusted code from the structured action arguments, never from
  model-written text.
- Read-only actions auto-run, so **no action may send data off the machine**
  (no generic HTTP fetch tool); repo/OBS lookups go through fixed endpoints.

### Model and runtime

- LLM listens on a Unix socket readable only by the agent group, not on TCP
  (localhost ports are reachable by other local users and, via DNS
  rebinding, by browsers).
- GGUF weights only (no pickle formats), shipped as an OBS package with
  checksums and signatures rather than downloaded ad hoc.

### Later

- SELinux policy module confining yaistd and llama-server (fits openSUSE's
  SELinux-by-default).
- Remote management: TUI over SSH works out of the box; no network listener
  in yaistd.

## Tech stack (Rust)

| Area | Choice |
|---|---|
| D-Bus (systemd, hostnamed, timedated, localed, firewalld, polkit, accounts) | `zbus` |
| Packages | phase 1: `zypper --xmlout --non-interactive`; later: libzypp via `cxx` shim — check Agama's Rust libzypp work first |
| Community repos | reuse/port `opi` logic (OBS search, repo add) |
| Migration / upgrades | wrap `opensuse-migration-tool`, `zypper dup`; `transactional-update` backend for immutable variants |
| SELinux | libselinux / `semanage`, `setsebool`, audit log parsing |
| TUI | `ratatui` |
| GUI | Slint or iced (decide after checking what ArtCraft uses) |
| LLM runtime | `llama.cpp` `llama-server` (packaged in Tumbleweed), Vulkan backend |

## Models

OSI-licensed (Apache 2.0 / MIT) only — no Llama/Gemma licences.

| Tier | Hardware | Candidates |
|---|---|---|
| Default | 8–16 GB RAM, CPU/iGPU | Qwen3-4B-Instruct-2507, Granite 4.0 Micro / H-Tiny |
| Better | 32 GB RAM or dGPU | Qwen3-30B-A3B-Instruct, gpt-oss-20b (MoE, ~3B active) |
| Fully open (incl. data) | — | OLMo, SmolLM3 |

Re-check newest releases at selection time; pick by eval results, not by vibes.

### Hitting ~5 s

- Instruct / non-thinking models (reasoning off).
- Static system prompt + tool schemas, reused via llama.cpp KV/prompt cache.
- Route first, then expose only one module's tools (5–10, not 60).
- Grammar/JSON-schema constrained decoding for tool calls.
- Templated result summaries for simple actions (skip the second LLM call).
- Stream tokens; show the plan card as soon as the tool call is parsed.
- Direct `/commands` bypass the LLM entirely.

## Workspace layout

```
crates/
  yaist-core     action registry, schemas, risk levels, plan/diff types
  yaistd         privileged daemon, D-Bus service, polkit, snapper, audit log
  yaist-modules  system, users, services, firewall, selinux, packages, repos, migration
  yaist-agent    prompt resolver: /command parser + LLM loop (llama-server client)
  yaist-ui       shared client view model (modules, panels, plan cards, conversation)
  yaist-tui      ratatui client
  yaist-gui      GUI client
  yaist-cli      scripting CLI
eval/            prompt → expected action dataset + latency benchmark
```

## Phases

0. **Spike** — eval set (~200 prompts → expected actions), model latency
   benchmarks on low/high-end reference hardware, zypper XML PoC.
1. **Read-only MVP** — daemon, read-only actions (system info, services, users,
   firewall zones, SELinux status, repos), TUI with prompt + `/commands`.
2. **Changes** — plan cards, confirmation, snapper, per-action polkit
   policy, audit log, sandboxed llama-server unit. Hostname,
   time/locale, users, services, firewalld, SELinux booleans/contexts, AVC
   denial explanations.
3. **GUI** — prompt + generated module panels for classic YaST-style use.
4. **Packages & repos** — search, install/remove, patches, community/OBS repos
   with trust prompts.
5. **Upgrades & migration** — Leap↔Leap/Tumbleweed via migration tool, dry-run
   summaries, conflict explanations, rollback.
6. **Later** — docs/wiki RAG, NetworkManager, bootloader.
7. **Nice to have: containers** (à la Cockpit) — Podman via its libpod REST
   API. Rootless containers go through the user's own podman socket, so no
   yaistd/root involvement; rootful ones through yaistd. Manage them as
   Quadlet units so they reuse the Services module (start/stop/enable/logs).

## Open questions

- "systemd firewall" assumed to mean **firewalld** (openSUSE default) — confirm.
- ~~GUI toolkit~~: egui (0.37 now), the same as the ArtCraft launcher.
- ~~Project licence~~: Apache-2.0 (see docs/LICENSING.md).
- Reference hardware for the 5 s target.
