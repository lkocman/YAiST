# YAiST system interfaces

YAiST reaches the system through **one set of typed actions**. People,
scripts and the language model all go through it, in different forms: a
button, a `/command`, a familiar `zypper …` line, a tool call. This document
describes those forms as an interface: what other front ends (another
client, a shell integration, a different assistant) can rely on, and what
a new module has to provide to be part of it.

The handbook (`docs/handbook/`) tells users how to *use* this; this
document says how it *works*. [PLAN.md](../PLAN.md) has the design
reasons and the parts that aren't built yet.

## Layers

```
 people              scripts                  the model
 buttons, forms      yaist run /…             tool calls (JSON)
 /commands, Tab      yaist help / tools
 zypper in …
        \                |                     /
         └──────── action registry ───────────┘     yaist-core, yaist-modules
                   validate · risk · plan card
                         |
            reads:       zypper --xmlout, systemd and firewalld on D-Bus, …
            changes:     pkexec <fixed binary> <argv>, D-Bus with polkit,
                         pkexec yaist-helper <operation>   (later: yaistd on D-Bus)
```

| Interface | For | Where |
|---|---|---|
| [Actions](#actions) | everything below | `crates/yaist-core/src/action.rs`, `registry.rs`; one file per module in `crates/yaist-modules/src/` |
| [`/command` language](#the-command-language) | people, scripts | `crates/yaist-core/src/command.rs` |
| [Familiar tool commands](#familiar-tool-commands) | people who know the terminal | `command::translate` |
| [Session commands](#session-commands) | people in a client | `crates/yaist-ui/src/session.rs` |
| [Completion](#completion) | people in a client | `command::complete`, `crates/yaist-ui/src/complete.rs` |
| [Tool calls](#tool-calls) | the language model | `crates/yaist-agent` |
| [CLI](#cli) | scripts, SSH | `apps/yaist/src/main.rs` |
| [Privileged operations](#privileged-operations) | YAiST itself | `apps/yaist/src/bin/yaist-helper` |

## Actions

An action is an `ActionSpec` plus a handler, registered in the module's
`register` function:

| Field | Meaning |
|---|---|
| `id` | `module.verb`, e.g. `firewall.open_port`. The module part is the module's id; the verb is `snake_case`. Ids are the stable name: tool names, commands and log lines derive from them. |
| `title` | Button label, e.g. "Open port". |
| `description` | What it does, for the model, tooltips and `/help`. |
| `risk` | `read`, `change` or `dangerous` (below). |
| `params` | Ordered list of parameters (below). The order is the positional order of the `/command`. |

**Risk** decides what happens on invocation:

- `read`: only reads state and runs straight away. **An action that
  changes the system is never `read`.**
- `change`: becomes a plan card and waits for the user's confirmation.
- `dangerous`: as `change`, marked in red: hard to undo, or changes what
  the system trusts (removing users, adding repositories, migrations).

**Parameters** (`Param`) have a name, a kind and a help text:

| Kind | Values | JSON schema |
|---|---|---|
| `text` | any string, usually with a validator | `string` |
| `integer` | a number | `integer` |
| `boolean` | `true`/`yes`/`y`/`1`/`on`, `false`/`no`/`n`/`0`/`off` | `boolean` |
| `choice` | one of a fixed list | `string` with `enum` |

A parameter is required unless it is `optional()` or has a `default(…)`.
`secret()` values are masked on plan cards, in commands and in logs;
`multiline()` gets a multi-line field in forms. A `check(validator)`
points to a function in `crates/yaist-core/src/validate.rs`. **Every
argument that reaches a command or D-Bus is validated there**, whoever sent
it. `ActionSpec::validate` turns raw strings into typed `Args`, and it
rejects unknown parameter names, so a confused model can't add any.

**Results** (`Output`) are one of:

- `Done(line)`: a finished change, in one line.
- `Text(text)`: free text.
- `Facts([(key, value)])`: a short key/value list.
- `Table`: columns (key and title) and rows. A module's overview table
  feeds its panel. Its row actions take their parameters from the row's
  columns of the same name.

**Questions**: a running action can ask the user something (a package
conflict, a license) with `yaist_core::ask::ask(Question)`. A question has
a title, the tool's own text, and choices, one of them the default. The
client in front answers it: a dialog in the GUI, a list in the TUI,
numbered choices with `yaist run`. With nobody to ask, the default applies.
Questions and answers go to the change's log.

**Terminal equivalent**: `Registry::shell(id, fn)` registers the shell
commands an action stands for. Plan cards show them, and so does the
generated *Shell recipe* in the handbook. They are for teaching and
reproducing, not what YAiST runs (privileged steps go through the helper).

## The `/command` language

Every action has a command:

```
/<module> <verb> [positional …] [name=value …]
```

- `<verb>` is the action's verb, with `-` or `_` (`open-port` and
  `open_port` are the same; YAiST writes `-`).
- Positional words fill the parameters that weren't named, in their
  declared order. `name=value` sets a parameter by name, in any position.
- Words split on white space; `"…"` and `'…'` quote, and `\` escapes inside
  double quotes.
- Too many words, an unknown verb or module, or a value the validator
  refuses are errors. Nothing runs.

```
/firewall open-port 8080            # protocol takes its default, tcp
/firewall open-port 8080 udp zone=public
/users create alice full_name="Alice Smith" admin=yes
/help                               # the modules
/help firewall                      # or /firewall: one module's commands
```

**Canonical form**: `command::format` writes the command for an action and
its arguments: required parameters positional, optional ones named, and
values equal to the default left out. Panels and the agent show this form for what
they did, and parsing it gives back the same arguments.

## Familiar tool commands

Lines that start with a known tool, with or without `/` or `sudo`, are
translated to the matching `/command` (`command::translate`): `zypper`,
`systemctl`, `snapper`, `firewall-cmd`, `hostnamectl`, `timedatectl`,
`setsebool`, `getsebool`, `sestatus`, `setenforce`, `useradd`, `userdel`.

```
zypper in fish                 →  /packages install fish
sudo systemctl enable --now sshd → /services enable sshd now=yes
firewall-cmd --add-port=8080/tcp →  /firewall open-port 8080 tcp
```

A subcommand YAiST doesn't know is an error that points to `/help`. It is
never passed to a shell. The tests in `command.rs` list the supported
forms.

## Session commands

Handled by the client session, not the registry. They change nothing on the
system:

| Command | Does |
|---|---|
| `/help [module]` | lists modules, or one module's commands (`/help` also mentions `/zoom`) |
| `/clear` | clears the conversation and the assistant's memory of it |
| `/zoom in\|out\|reset\|150%` | text size (GUI) |
| `/demo [scene]` | made-up cards for screenshots; not in `/help` |

`yaist_ui::complete::CLIENT_COMMANDS` lists them with their argument words.
A new session command goes there too, so that Tab knows it.

## Completion

`command::complete(registry, line, client_commands)` completes the last
word of a line (the text before the cursor). It returns where that word
starts and the whole words that can stand there:

| After | Candidates |
|---|---|
| `/` | `/help`, every `/<module>`, the session commands (sorted) |
| `/help ` | module ids |
| `/<session-command> ` | its argument words |
| `/<module> ` | the module's verbs (sorted) |
| `/<module> <verb> …` | values of the next free parameter when it is a `choice` or `boolean` (`yes`, `no`), then `name=` for each free parameter, in declared order |
| `… name=` | `name=<value>` for a `choice` or `boolean` parameter |

Free text (`text`, `integer`) and plain-language requests complete to
nothing.

Clients use `yaist_ui::complete::Completer`, so Tab works the same in all
of them:

1. Tab on a line starting with `/` completes. On any other line, Tab moves
   the focus as before.
2. One candidate: it replaces the word and a space follows (no space after
   `name=`).
3. Several: the word grows to their common start.
4. Nothing to add: the candidates are listed (under the prompt in the TUI,
   above the input in the GUI).
5. Tab again picks them in turn, Shift+Tab goes back, and in the GUI a
   click picks one. Typing anything else ends the round.

## Tool calls

The agent offers every action to the model as a tool:

- name: the action id with `.` replaced by `__` (`firewall__open_port`);
  `yaist_agent::action_id` maps it back.
- description: the action's `description`.
- parameters: `ActionSpec::json_schema()`: the parameter kinds above, with
  the default added to each description.

`yaist tools` prints them all as JSON. A `read` tool call runs, and its
`Output` goes back to the model as plain text (tables cut to 40 rows). A
`change` or `dangerous` call becomes a plan card like any other. The
model's call is answered when the user confirms or cancels, and the outcome
follows as a note. The model never gets a shell. Tool output is treated as untrusted data.

One tool is not an action: `text_size`, which the client handles (for
showing YAiST on a shared screen).

## CLI

```
yaist                          the terminal client
yaist run /firewall overview   run one command (the leading / is optional)
yaist run /… --yes             changes need --yes; without it, YAiST prints
                               the canonical command and stops
yaist ask "open port 8080"     one request through the agent; plans are
                               shown, never applied
yaist help [module]            the /help text
yaist tools                    the tool definitions (JSON)
yaist docs [--check]           regenerate the handbook's generated blocks
```

`yaist run` prints tables as aligned columns and other results as text. It
asks a running change's questions on the terminal.

## Privileged operations

Clients run unprivileged. A change that is one root command runs it with
`Context::run_privileged`: `pkexec`, a fixed binary path and an argv,
never a shell (`/usr/sbin/useradd --create-home -- alice`). systemd and
firewalld changes go over their D-Bus APIs, which ask polkit themselves.
Operations of several steps, or ones that need to check their own input
as root, go through `pkexec yaist-helper <operation> …`. The helper runs
one fixed operation per invocation and **validates every argument again
itself**, since whoever starts it is not trusted:

```
snapper-list | snapper-create <description> | snapper-status <n>
snapper-delete <n> | snapper-rollback <n>
migrate-leap leap|leap-micro <version> [repo-alias-to-disable …]
migrate-rolling tumbleweed|slowroll [repo-alias-to-disable …]
migrate-sle <email> <regcode-file> [repo-alias-to-disable …]
nvidia-setup leap|tumbleweed
codecs-packman
```

polkit is the security gate. The client's *Confirm* button only lets the user review
first. The planned `yaistd` daemon replaces the helper with a D-Bus API,
with one polkit action per kind of change; see PLAN.md.

## Adding to the interface

A new action, in its module's file under `crates/yaist-modules/src/`:

1. An `ActionSpec` with an id `module.verb`, the right `Risk`, and
   parameters in the order a person would type them.
2. Validators in `validate.rs` for every text that reaches a command.
3. The handler, returning an `Output`, and if there is one, the terminal
   equivalent with `Registry::shell`.
4. If root is needed: an operation in `yaist-helper` that validates its
   arguments again.
5. `yaist docs` to regenerate the handbook's action blocks, and the prose in
   the module's chapter.

Commands, completion, forms, tool schemas and `/help` follow from the spec.
None of them has to be written by hand.

## Stability

YAiST is a prototype. The `/command` syntax, action ids, risk levels and
tool names are the parts meant to stay. Scripts and other front ends should
use those. Rust types, the helper's operations and session commands may
still change.
