#!/bin/bash
# A clean, throwaway openSUSE Leap 16.0 container with YAiST mounted at /yaist,
# for trying changes and migrations as root. Options: see clean-container.sh
# (--keep, --update, --release, --no-pull, -- CMD...).
exec "$(dirname "$(readlink -f "$0")")/clean-container.sh" leap160 "$@"
