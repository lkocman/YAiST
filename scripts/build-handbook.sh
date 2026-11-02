#!/bin/bash
# Builds the openSUSE Handbook into target/handbook in a throwaway
# Tumbleweed container, for machines without the tools. Arguments pick
# formats (default: pdf epub html). Update the generated parts first with
# `yaist docs`. The build itself is scripts/handbook-build.sh, as in CI.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p target/handbook
exec podman run --rm --userns=keep-id --user root -v "$PWD":/src:Z -w /src \
    -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
    registry.opensuse.org/opensuse/tumbleweed:latest sh -euc '
        scripts/handbook-build.sh --install target/handbook "$@" >/dev/null
        chown -R "$HOST_UID:$HOST_GID" target/handbook
        ls target/handbook
    ' sh "$@"
