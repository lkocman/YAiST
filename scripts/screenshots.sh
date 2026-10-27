#!/bin/bash
# Regenerates the README screenshots in docs/screenshots: the GUI runs in a
# throwaway Tumbleweed container (as root there, so zypper needs no polkit)
# and draws on the desktop's Wayland session. The package conflict is a real
# one, raised by zypper. Needs a built target/debug/yaist-gui (e.g.
# scripts/build-in-container.sh build), Wayland, ImageMagick; the local
# model (scripts/llm-server.sh) is optional but shows up in the header.
# Shots that are hard to reach for real (a failed change) are made-up
# scenes, see crates/yaist-ui/src/demo.rs. Arguments pick the shots to take
# (e.g. scripts/screenshots.sh report); none takes them all.
set -euo pipefail
cd "$(dirname "$0")/.."
NAME=yaist-screenshots
WANTED=("$@")
OUT=$(mktemp -d)
trap 'podman rm -f "$NAME" >/dev/null 2>&1; rm -rf "$OUT"' EXIT

podman run -d --name "$NAME" --network host --security-opt label=disable --device /dev/dri \
    -v "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY":/tmp/wl/wayland-0 -v "$PWD":/src:ro -v "$OUT":/shots \
    -e XDG_RUNTIME_DIR=/tmp/wl -e WAYLAND_DISPLAY=wayland-0 \
    -e YAIST_FONT_DIR=/src/assets/fonts -e YAIST_THEME=light \
    registry.opensuse.org/opensuse/tumbleweed:latest sleep infinity >/dev/null
podman exec "$NAME" bash -c 'zypper -q refresh && zypper -q -n in --no-recommends \
    Mesa-dri Mesa-libEGL1 Mesa-libGL1 libwayland-client0 libwayland-egl1 libwayland-cursor0 libxkbcommon0' >/dev/null

shot() { # name delay [env...]
    local name=$1 delay=$2
    shift 2
    if [ ${#WANTED[@]} -gt 0 ] && [[ ! " ${WANTED[*]} " =~ " $name " ]]; then
        return
    fi
    podman exec -e YAIST_SCREENSHOT="/shots/$name.ppm" -e YAIST_SCREENSHOT_DELAY="$delay" "$@" \
        "$NAME" timeout 120 /src/target/debug/yaist-gui
    magick "$OUT/$name.ppm" -strip -define png:compression-level=9 "docs/screenshots/$name.png"
    echo "docs/screenshots/$name.png"
}

mkdir -p docs/screenshots
shot welcome 25
shot package-conflict 30 -e YAIST_DEMO_CONFIRM=1 \
    -e "YAIST_DEMO=zypper in postfix sendmail exim pipewire-pulseaudio pulseaudio busybox-which which"
shot report 8 -e YAIST_DEMO_SCENE=report-distro
