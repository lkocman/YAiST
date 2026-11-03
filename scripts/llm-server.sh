#!/bin/bash
# Runs llama.cpp's server. The packaged llama-server (llamacpp with
# libggml-vulkan) comes first: it uses the host's Vulkan drivers, NVIDIA's
# included, and on computers with two GPUs it starts through switcherooctl on
# the discrete one. Without it, the server runs in a rootless podman
# container, which only sees Mesa GPUs through /dev/dri. The model is
# downloaded on first start.
#
#   YAIST_MODEL      Hugging Face repo[:quant]  (default: Qwen3-4B-Instruct-2507 Q4_K_M)
#   YAIST_LLM_PORT   port on 127.0.0.1          (default: 8080)
#   YAIST_LLM_CTX    context size in tokens     (default: 16384; 8192 fits the
#                                                prompt and tools on small GPUs)
#   YAIST_LLM_IMAGE  container image            (default: llama.cpp Vulkan build)
#   YAIST_LLM_PODMAN set to 1 to use the container even when llama-server is installed
#
# GPU layers are not set: llama-server's --fit (on by default) offloads as
# many as fit in the GPU's memory and keeps the rest on the CPU.
set -euo pipefail

MODEL="${YAIST_MODEL:-unsloth/Qwen3-4B-Instruct-2507-GGUF:Q4_K_M}"
PORT="${YAIST_LLM_PORT:-8080}"
CTX="${YAIST_LLM_CTX:-16384}"
IMAGE="${YAIST_LLM_IMAGE:-ghcr.io/ggml-org/llama.cpp:server-vulkan}"
CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/yaist/models"

mkdir -p "$CACHE"

if [ -r /proc/meminfo ]; then
    mem_total_kb=$(awk '/MemTotal:/ {print $2}' /proc/meminfo 2>/dev/null || true)
    if [ -n "$mem_total_kb" ] && [ "$mem_total_kb" -lt 3800000 ]; then
        echo "llm-server: system has less than 4 GiB RAM; local model is disabled to prevent OOM freeze." >&2
        exit 1
    fi
fi

server_args=(-hf "$MODEL" --jinja -c "$CTX" --cache-reuse 256)

gpu_count() {
    local n=0 card
    for card in /sys/class/drm/card[0-9]*; do
        [[ $card == *-* ]] && continue
        [ -e "$card/device/vendor" ] && n=$((n + 1))
    done
    echo "$n"
}

if [ "${YAIST_LLM_PODMAN:-0}" != 1 ] && command -v llama-server >/dev/null; then
    launch=()
    if [ "$(gpu_count)" -ge 2 ]; then
        if grep -qs "0x10de" /sys/class/drm/card[0-9]*/device/vendor 2>/dev/null && [ ! -d /sys/module/nvidia ]; then
            echo "llm-server: dual-GPU system with NVIDIA detected, but the NVIDIA driver is not loaded." >&2
            echo "            Set it up in YAiST (AI assistant → Set up) or install the driver and reboot." >&2
            exit 1
        fi
        # switcherooctl puts the default (integrated) GPU first, so -g 1 is
        # the discrete one. When its service isn't running it starts the
        # command without a word on the default GPU, so check first.
        if ! command -v switcherooctl >/dev/null; then
            echo "llm-server: two GPUs but switcheroo-control isn't installed; the model would run on the integrated one." >&2
            echo "            Set it up in YAiST (AI assistant → Set up), or: sudo zypper install switcheroo-control" >&2
            exit 1
        fi
        if ! busctl --system status net.hadess.SwitcherooControl >/dev/null 2>&1; then
            echo "llm-server: switcheroo-control isn't running; the model would run on the integrated GPU." >&2
            echo "            Start it: sudo systemctl enable --now switcheroo-control" >&2
            exit 1
        fi
        launch=(switcherooctl -g 1)
    fi
    export LLAMA_CACHE="$CACHE"
    exec "${launch[@]}" llama-server "${server_args[@]}" --host 127.0.0.1 --port "$PORT"
fi

gpu=()
if [ -e /dev/dri ]; then
    gpu=(--device /dev/dri)
fi

exec podman run --rm --name yaist-llm \
    "${gpu[@]}" \
    --cap-drop all --security-opt no-new-privileges \
    -e LLAMA_CACHE=/models \
    -v "$CACHE":/models:Z \
    -p "127.0.0.1:$PORT:8080" \
    "$IMAGE" \
    "${server_args[@]}" \
    --host 0.0.0.0 --port 8080
