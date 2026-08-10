# stt plugin

Speech-to-text for Veyron plugins. Exposes two actions: `stt_transcribe`
(turn audio into text) and `stt_models` (list transcribable models).

Two providers behind one normalized interface:

| Provider | Where it runs | What it is |
|---|---|---|
| `sherpa` | **in-process** (local) | sherpa-onnx ONNX inference — zipformer/whisper offline models, fully offline |
| `openai` | cloud, via `network` | OpenAI Audio API (`whisper-1` / `gpt-4o-transcribe` / `gpt-4o-mini-transcribe`) |

The cloud provider routes every request through the `network` plugin's
`http_request` action, so `network` must also be registered and running
for it (same model as `ai` and `tts`). `sherpa` opens no sockets — it
loads an ONNX model from disk and transcribes in-process, so it works
with nothing but the kernel and the model files.

**See [`USAGE.md`](./USAGE.md)** for the caller-facing guide: full
`stt_transcribe` / `stt_models` request/response reference, per-provider
examples, every error message a caller can hit, and common patterns.

## Operator note

`stt` declares zero kernel permissions (`plugin.json`: `"permissions": []`)
and opens no sockets itself, so it's safe to run with `sandbox: true`.
`network` still needs `sandbox: false` (real egress) for the cloud
provider — see `plugins/network/README.md`.

The local provider loads a model into RAM at first use; size `max_vmem_mb`
above the model size (zipformer int8 ≈ 50-100 MB; whisper tiny/base a few
hundred MB). See `config.example.yaml`.

## Action: `stt_transcribe`

Request (`ActionRequest.params_json`):

```json
{
  "provider": "sherpa",
  "audio_base64": "UklGRgAAAABXQVZF..."
}
```

- `provider` — `"sherpa"` | `"openai"`. Required.
- `audio_base64` — required, base64 of the audio bytes, ≤ 25 MiB after
  decoding.
- `format` — optional. `sherpa`: `wav` (default) | `pcm` (raw 16-bit,
  requires `sample_rate_hz` + `num_channels`); `openai`: `wav` (default) |
  `mp3` | `ogg`.
- `sample_rate_hz`, `num_channels` — required for `sherpa` with `pcm`
  format; ignored otherwise.
- `language` — optional ISO-639-1 hint (e.g. `"de"`). Caller-declared;
  echoed back in the response, and sent to the provider for `openai` /
  applied per-request for `sherpa` whisper models.
- `prompt` — optional Whisper-style context hint (`openai` only), ≤ 1000
  chars.
- `temperature` — optional, `0.0`..=`1.0` (`openai` only).
- `api_key_env` — required for `openai` (name of an env var the `stt`
  process reads at call time, never a literal key; must be on the
  operator's `STT_PLUGIN_ALLOWED_KEY_ENVS` allowlist). Ignored for `sherpa`.
- `timeout_ms` — optional, default/cap `60000`. Cloud requests are
  additionally capped at `network`'s own 30 s HTTP limit.
- `base_url`, `model` — optional per-provider overrides (defaults:
  `https://api.openai.com/v1` / `whisper-1`; for `openai`, `model` must be
  one of the ids `stt_models` lists).

Response (`ActionResponse.data_json`) on success, normalized across both
providers:

```json
{
  "text": "Hello from Veyron.",
  "language": "en",
  "duration_seconds": 2.4,
  "model": "sherpa:transducer"
}
```

`language` is `""` when unknown; `duration_seconds` is `0` when it can't
be derived from the container format (e.g. an mp3/ogg upload). `model` is
the provider's model id (for `openai`, the resolved model; for `sherpa`,
`sherpa:<family>`).

Errors → `ACTION_ERROR` with a human-readable message: malformed/missing
request fields, unknown model, model load failure (missing/wrong model
files), un-allowlisted or unset `api_key_env`, non-2xx HTTP status from a
provider, or any error `network`'s `http_request` itself returns.

## Action: `stt_models`

```json
{ "provider": "sherpa" }
```

Returns the models the provider exposes:

```json
[
  { "id": "sherpa:transducer", "name": "local sherpa-onnx model (transducer)" }
]
```

- `sherpa` — the single operator-configured model.
- `openai` — the known model id list (`whisper-1`, `gpt-4o-transcribe`,
  `gpt-4o-mini-transcribe`).

## Configuration

`stt` reads no config file itself — everything is environment variables
set in the kernel's `config.yaml`, under this plugin's `env:` list — see
`config.example.yaml` in this directory.

- `STT_PLUGIN_ALLOWED_KEY_ENVS` — **required for the cloud provider**: a
  comma-separated, exact-match allowlist of every env var name a caller's
  `api_key_env` may reference. Default-deny — without it every cloud
  `stt_transcribe` request is rejected. Same rationale as `ai`'s
  `AI_PLUGIN_ALLOWED_KEY_ENVS` and `tts`'s `TTS_PLUGIN_ALLOWED_KEY_ENVS`.
- `STT_PLUGIN_LOCAL_MODEL_DIR` — **required for `sherpa`**: directory with
  the ONNX model files.
- `STT_PLUGIN_LOCAL_MODEL_TYPE` — **required for `sherpa`**: `transducer`
  or `whisper`.
- `STT_PLUGIN_LOCAL_NUM_THREADS` — optional, default `2`.
- `STT_PLUGIN_LOCAL_LANGUAGE` — optional (whisper family only), default
  `"en"`.

### Setting up a local model

**Transducer (zipformer)** — solid accuracy for English and several other
languages; medium-sized. Create `STT_PLUGIN_LOCAL_MODEL_DIR` with:

```
encoder.onnx         # from a sherpa-onnx-zipformer-* model pack
decoder.onnx
joiner.onnx
tokens.txt
```

**Whisper** — classic Whisper accuracy, converted to ONNX. Create the dir
with:

```
encoder.onnx         # from a sherpa-onnx-whisper-* model pack
decoder.onnx
tokens.txt
```

Model packs download from the k2-fsa/sherpa-onnx releases
(`sherpa-onnx-zipformer-en-2023-06-26`, `sherpa-onnx-whisper-tiny.en`,
etc.). The model is loaded lazily on the first `sherpa` transcribe request
and cached for the process lifetime.

## Testing

`cargo test` — 57 unit tests, no live network and no model files required
(providers are tested against fixture audio/JSON; sherpa config assembly
is tested without loading a real model). There's no automated
kernel + `network` + model integration test yet.
