#!/bin/bash
# Runs llama.cpp's server. The packaged llama-server (llamacpp with
# libggml-vulkan) comes first: it uses the host's Vulkan drivers, NVIDIA's
# included, and on computers with two GPUs it starts through switcherooctl on
# the discrete one. Without it, the server runs in a rootless podman
# container, which only sees Mesa GPUs through /dev/dri. The model is
# downloaded on first start.
#
#   YAIST_MODEL      Hugging Face repo[:quant]  (default: Qwen3-4B-Instruct-2507 Q4_K_M)
#   YAIST_MODEL_FILE a GGUF file to serve instead, e.g. the one yaist-offline
#                    installs; needs the packaged llama-server
#   YAIST_LLM_ALIAS  the model name the server reports (default: YAIST_MODEL,
#                    or the file name)
#   YAIST_LLM_PORT   port on 127.0.0.1          (default: 8080)
#   YAIST_LLM_CTX    context size in tokens     (default: 16384; 8192 fits the
#                                                prompt and tools on small GPUs)
#   YAIST_LLM_IMAGE  container image            (default: llama.cpp Vulkan build)
#   YAIST_LLM_PODMAN set to 1 to use the container even when llama-server is installed
#   YAIST_LLM_DEVICE auto, gpu or cpu           (default: auto)
#
# GPU layers are not set: llama-server's --fit (on by default) offloads as
# many as fit in the GPU's memory and keeps the rest on the CPU.
#
# CPU mode: without a usable discrete GPU (NVIDIA with its driver loaded, or
# an AMD card with its own memory) the model runs on the processor. llama.cpp
# would otherwise pick the integrated GPU, which is slower than the CPU on
# most laptops and sometimes fails. The alias gets "@cpu" so YAiST can show
# that it runs in CPU mode. YAIST_LLM_DEVICE=gpu overrides this, for example
# on strong integrated GPUs. Keep in step with crates/yaist-modules/src/assistant.rs.
set -euo pipefail

MODEL="${YAIST_MODEL:-unsloth/Qwen3-4B-Instruct-2507-GGUF:Q4_K_M}"
PORT="${YAIST_LLM_PORT:-8080}"
CTX="${YAIST_LLM_CTX:-16384}"
IMAGE="${YAIST_LLM_IMAGE:-ghcr.io/ggml-org/llama.cpp:server-vulkan}"
MODEL_FILE="${YAIST_MODEL_FILE:-}"

if [ -n "$MODEL_FILE" ]; then
    # yaist-llm.service: a packaged model, nothing to download or cache.
    ALIAS="${YAIST_LLM_ALIAS:-$(basename "$MODEL_FILE" .gguf)}"
    server_args=(-m "$MODEL_FILE" --jinja -c "$CTX" --cache-reuse 256)
else
    ALIAS="${YAIST_LLM_ALIAS:-$MODEL}"
    CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/yaist/models"
    mkdir -p "$CACHE"
    export LLAMA_CACHE="$CACHE"
    server_args=(-hf "$MODEL" --jinja -c "$CTX" --cache-reuse 256)
fi

# The vendor of a discrete GPU llama.cpp can use: "nvidia" or "amd", or nothing.
usable_discrete_gpu() {
    local card dev driver vram
    for card in /sys/class/drm/card[0-9]*; do
        [[ $card == *-* ]] && continue
        dev=$card/device
        [ -e "$dev/vendor" ] || continue
        driver=$(basename "$(readlink -f "$dev/driver")")
        case $(<"$dev/vendor") in
        0x10de)
            # nouveau has no usable Vulkan on most cards
            [ "$driver" = nvidia ] && { echo nvidia; return; } ;;
        0x1002)
            # An APU reports only its small carve-out as VRAM.
            vram=$(cat "$dev/mem_info_vram_total" 2>/dev/null || echo 0)
            [ "$driver" = amdgpu ] && [ "$vram" -ge $((4 << 30)) ] && { echo amd; return; } ;;
        esac
    done
    return 0
}

gpu_count() {
    local n=0 card
    for card in /sys/class/drm/card[0-9]*; do
        [[ $card == *-* ]] && continue
        [ -e "$card/device/vendor" ] && n=$((n + 1))
    done
    echo "$n"
}

dgpu=$(usable_discrete_gpu)
case "${YAIST_LLM_DEVICE:-auto}" in
cpu) cpu=1 ;;
gpu) cpu=0 ;;
*) [ -n "$dgpu" ] && cpu=0 || cpu=1 ;;
esac
cpu_mode() {
    echo "llm-server: no usable discrete GPU, running the model on the CPU (slower; YAIST_LLM_DEVICE=gpu overrides)." >&2
    server_args+=(--device none)
    ALIAS="$ALIAS@cpu"
}

if [ "${YAIST_LLM_PODMAN:-0}" != 1 ] && command -v llama-server >/dev/null; then
    launch=()
    if [ "$cpu" = 1 ]; then
        cpu_mode
    elif [ "$(gpu_count)" -ge 2 ]; then
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
    exec "${launch[@]}" llama-server "${server_args[@]}" --alias "$ALIAS" --host 127.0.0.1 --port "$PORT"
fi

if [ -n "$MODEL_FILE" ]; then
    echo "llm-server: YAIST_MODEL_FILE needs the packaged llama-server: sudo zypper install llamacpp" >&2
    exit 1
fi

# The container only sees Mesa GPUs, so NVIDIA's driver doesn't count here.
gpu=()
if [ "${YAIST_LLM_DEVICE:-auto}" = auto ] && [ "$dgpu" != amd ]; then
    cpu=1
fi
if [ "$cpu" = 1 ]; then
    cpu_mode
elif [ -e /dev/dri ]; then
    gpu=(--device /dev/dri)
fi

exec podman run --rm --name yaist-llm \
    "${gpu[@]}" \
    --cap-drop all --security-opt no-new-privileges \
    -e LLAMA_CACHE=/models \
    -v "$CACHE":/models:Z \
    -p "127.0.0.1:$PORT:8080" \
    "$IMAGE" \
    "${server_args[@]}" --alias "$ALIAS" \
    --host 0.0.0.0 --port 8080
