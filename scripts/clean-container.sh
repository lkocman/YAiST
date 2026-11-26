#!/bin/bash
# A clean, throwaway openSUSE container for trying YAiST's changes as root,
# above all the migrations (Leap 16.0 -> 16.1, Leap -> Tumbleweed), without
# touching this machine. Usually started through a wrapper:
#
#   scripts/clean-leap160-container.sh
#   scripts/clean-leap161-container.sh
#   scripts/clean-tumbleweed-container.sh
#
# or directly: scripts/clean-container.sh leap160|leap161|tumbleweed [options]
#
# Every run starts from a fresh image pull: an older container of the same
# name is removed first. The YAiST binaries you built (target/debug, or
# target/release with --release) are mounted read-only at /yaist and are on
# PATH, so inside the container `yaist run /upgrade targets` or
# `yaist run /upgrade migrate leap:16.1 --yes` runs as root, without pkexec.
# Build them first, e.g. scripts/build-in-container.sh build (Leap 16.0
# binaries run on 16.1 and Tumbleweed as well).
#
# With --gui the YAiST desktop client starts straight away, as root (no
# pkexec), drawing on this desktop's Wayland session; it installs the few
# Mesa/Wayland libraries the GUI needs first. Open a second shell next to it
# with `podman exec -it <name> bash -l`.
#
# Options:
#   --gui         start yaist-gui on this desktop instead of a shell
#   --keep        keep the container after the shell exits (default: remove it);
#                 get back in with `podman start -ai <name>`
#   --update      bring the system up to date first (zypper refresh and
#                 update, or dup on Tumbleweed): migrations start from there
#   --release     mount target/release instead of target/debug
#   --no-pull     use the image already here instead of pulling a fresh one
#   -- CMD...     run CMD instead of an interactive shell (or the GUI)
#
# The container has no systemd, so actions that need D-Bus services (systemd
# units, firewalld, hostnamed) fail in it; package, repository and migration
# actions work.
set -euo pipefail
# Resolved before the cd: $0 may be relative to where we were started.
self=$(readlink -f "$0")
cd "$(dirname "$self")/.."

usage() {
    sed -n '2,/^set -euo/p' "$self" | sed '$d; s/^# \{0,1\}//'
    exit "${1:-0}"
}

[ $# -gt 0 ] || usage 1
case "$1" in
    leap160) image=registry.opensuse.org/opensuse/leap:16.0 ;;
    leap161) image=registry.opensuse.org/opensuse/leap:16.1 ;;
    tumbleweed) image=registry.opensuse.org/opensuse/tumbleweed:latest ;;
    -h | --help) usage ;;
    *) echo "unknown release '$1' (leap160, leap161 or tumbleweed)" >&2; exit 1 ;;
esac
release=$1
shift

keep=no update=no build=debug pull=yes gui=no
cmd=()
while [ $# -gt 0 ]; do
    case "$1" in
        --gui) gui=yes ;;
        --keep) keep=yes ;;
        --update) update=yes ;;
        --release) build=release ;;
        --no-pull) pull=no ;;
        -h | --help) usage ;;
        --) shift; cmd=("$@"); break ;;
        *) echo "unknown option '$1'" >&2; usage 1 ;;
    esac
    shift
done

name=yaist-clean-$release
bin=$PWD/target/$build
if [ ! -x "$bin/yaist" ]; then
    hint="scripts/build-in-container.sh build"
    [ "$build" = debug ] || hint="$hint --release"
    echo "note: $bin/yaist is missing; build it first ($hint)." >&2
    echo "      Starting without YAiST." >&2
    mkdir -p "$bin"
fi

gui_opts=()
if [ "$gui" = yes ]; then
    if [ -z "${WAYLAND_DISPLAY:-}" ] || [ ! -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ]; then
        echo "--gui needs a Wayland session (WAYLAND_DISPLAY is not set or its socket is missing)." >&2
        exit 1
    fi
    # Same setup as scripts/screenshots.sh: the Wayland socket, the GPU, and
    # the host network so a local model (scripts/llm-server.sh) is reachable.
    gui_opts=(--network host --device /dev/dri
        -v "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY":/tmp/wl/wayland-0
        -e XDG_RUNTIME_DIR=/tmp/wl -e WAYLAND_DISPLAY=wayland-0)
    [ -z "${YAIST_THEME:-}" ] || gui_opts+=(-e YAIST_THEME="$YAIST_THEME")
    [ -z "${YAIST_LLM_URL:-}" ] || gui_opts+=(-e YAIST_LLM_URL="$YAIST_LLM_URL")
    [ ${#cmd[@]} -gt 0 ] || cmd=(yaist-gui)
fi

if podman container exists "$name"; then
    echo "Removing the old $name container..."
    podman rm -f "$name" >/dev/null
fi
if [ "$pull" = yes ]; then
    podman pull "$image"
fi

# What runs inside: optionally update, add the GUI libraries, then the shell,
# the GUI or the given command.
prepare='set -e
echo "PATH=/yaist:\$PATH" >/etc/profile.d/zz-yaist.sh
if [ "$YAIST_UPDATE" = yes ] || [ "$YAIST_GUI" = yes ]; then
    zypper -n --gpg-auto-import-keys refresh
fi
if [ "$YAIST_UPDATE" = yes ]; then
    if grep -q "^ID=\"\?opensuse-tumbleweed" /etc/os-release; then
        zypper -n dup
    else
        zypper -n update
    fi
fi
if [ "$YAIST_GUI" = yes ]; then
    echo "Installing the libraries the GUI needs..."
    zypper -q -n in --no-recommends Mesa-dri Mesa-libEGL1 Mesa-libGL1 \
        libwayland-client0 libwayland-egl1 libwayland-cursor0 libxkbcommon0 >/dev/null
fi
. /etc/os-release
if [ "$YAIST_GUI" = yes ]; then
    echo "Clean $PRETTY_NAME container, starting $1. Shell next to it: podman exec -it $HOSTNAME bash -l"
else
    echo "Clean $PRETTY_NAME container. YAiST is in /yaist (on PATH); exit to leave."
fi
exec "$@"'
[ ${#cmd[@]} -gt 0 ] || cmd=(/bin/bash -l)

rm_flag=(--rm)
[ "$keep" = no ] || rm_flag=()
tty=(-i)
[ ! -t 0 ] || tty=(-it)

# No SELinux relabelling of target/: build-in-container.sh relabels the
# whole checkout for itself, and the two would lock each other out.
exec podman run "${rm_flag[@]}" "${tty[@]}" \
    --name "$name" --hostname "$name" \
    --security-opt label=disable \
    -v "$bin":/yaist:ro \
    -e PATH=/yaist:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    -e YAIST_UPDATE="$update" \
    -e YAIST_GUI="$gui" \
    "${gui_opts[@]}" \
    "$image" /bin/bash -c "$prepare" prepare "${cmd[@]}"
