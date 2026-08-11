#!/usr/bin/env bash
# Download the local ONNX models for the tts and stt sherpa providers.
#
# Installs two models (see plugins/tts/README.md and plugins/stt/README.md,
# "Setting up a local model", for the exact file layouts the providers
# require):
#
#   TTS (piper, ru_RU denis medium):
#     model.onnx, tokens.txt, espeak-ng-data/
#   STT (transducer, zipformer ru int8):
#     encoder.onnx, decoder.onnx, joiner.onnx, tokens.txt
#
# Sources are the official k2-fsa/sherpa-onnx release packs. The piper voice
# is the same ru_RU-denis-medium voice as the rhasspy/piper-voices HF pack,
# pre-assembled with tokens.txt and espeak-ng-data/ so the sherpa provider
# can load it as-is.
#
# Default install dirs are repo-local (models/tts, models/stt); override
# with TTS_PLUGIN_LOCAL_MODEL_DIR / STT_PLUGIN_LOCAL_MODEL_DIR to match a
# deployed layout (e.g. /opt/tts-models, /opt/stt-models).
#
# Idempotent: skips a model when its required files already exist.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

tts_dir="${TTS_PLUGIN_LOCAL_MODEL_DIR:-$repo_root/models/tts/piper-ru_RU-denis-medium}"
stt_dir="${STT_PLUGIN_LOCAL_MODEL_DIR:-$repo_root/models/stt/zipformer-ru-int8}"

tts_pack="vits-piper-ru_RU-denis-medium.tar.bz2"
tts_url="https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/$tts_pack"
stt_pack="sherpa-onnx-zipformer-ru-int8-2025-04-20.tar.bz2"
stt_url="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/$stt_pack"

tts_assets=(model.onnx tokens.txt espeak-ng-data)
tts_renames="ru_RU-denis-medium.onnx:model.onnx"
stt_assets=(encoder.onnx decoder.onnx joiner.onnx tokens.txt)
stt_renames="encoder.int8.onnx:encoder.onnx,joiner.int8.onnx:joiner.onnx"

# have_all <dir> <asset...> — every asset exists (files and dirs alike).
have_all() {
    local dir="$1"
    shift
    local f
    for f in "$@"; do
        [[ -e "$dir/$f" ]] || return 1
    done
}

# fetch <name> <url> <dest_dir> <renames> <asset...>
# renames: comma-separated "src:dst" pairs applied after extraction so packs
# with non-canonical filenames (ru_RU-denis-medium.onnx, encoder.int8.onnx)
# land where the sherpa provider expects them; "-" for none.
fetch() {
    local name="$1" url="$2" dest="$3" renames="$4"
    shift 4
    if have_all "$dest" "$@"; then
        echo "==> $name: already present in $dest, skipping"
        return 0
    fi
    local tmp
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' RETURN
    echo "==> $name: downloading $url"
    curl -fL --retry 3 --retry-delay 2 -o "$tmp/archive.tar.bz2" "$url"
    mkdir -p "$dest"
    echo "==> $name: extracting into $dest"
    tar -xjf "$tmp/archive.tar.bz2" --strip-components=1 -C "$dest"
    local pair src dst
    IFS=',' read -ra pairs <<< "$renames"
    for pair in "${pairs[@]}"; do
        [[ "$pair" == "-" ]] && continue
        src="${pair%%:*}"
        dst="${pair##*:}"
        if [[ -f "$dest/$src" && ! -e "$dest/$dst" ]]; then
            mv "$dest/$src" "$dest/$dst"
        fi
    done
    if ! have_all "$dest" "$@"; then
        echo "error: $name extract did not produce required assets in $dest" >&2
        exit 1
    fi
    echo "==> $name: done"
}

fetch "tts piper ru_RU-denis-medium" "$tts_url" "$tts_dir" "$tts_renames" "${tts_assets[@]}"
fetch "stt zipformer ru int8" "$stt_url" "$stt_dir" "$stt_renames" "${stt_assets[@]}"

echo
echo "Set in the kernel config.yaml:"
echo "  TTS_PLUGIN_LOCAL_MODEL_DIR=$tts_dir"
echo "  TTS_PLUGIN_LOCAL_MODEL_TYPE=piper"
echo "  STT_PLUGIN_LOCAL_MODEL_DIR=$stt_dir"
echo "  STT_PLUGIN_LOCAL_MODEL_TYPE=transducer"
