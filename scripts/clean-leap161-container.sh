#!/bin/bash
# A clean, throwaway openSUSE Leap 16.1 container with YAiST mounted at /yaist,
# for trying changes and migrations as root. Options: see clean-container.sh
# (--keep, --update, --release, --no-pull, -- CMD...).
exec "$(dirname "$0")/clean-container.sh" leap161 "$@"
