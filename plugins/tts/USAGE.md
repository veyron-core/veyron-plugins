# tts plugin — caller's guide

Everything a plugin (or the kernel) needs to speak to the `tts` plugin.
Actions: `tts_synthesize`, `tts_voices`.

## `tts_synthesize`

Turn text into audio bytes.

### Request

```json
{
  "provider": "sherpa",
  "text": "Hello from Veyron.",
  "voice": "af_heart",
  "format": "wav",
  "speed": 1.0,
  "timeout_ms": 60000
}
```

| Field | Required | Meaning |
|---|---|---|
| `provider` | yes | `sherpa` (local) \| `openai` \| `elevenlabs` (cloud) |
| `text` | yes | 1–4000 chars; trimmed |
| `voice` | yes | provider-specific id (below) |
| `api_key_env` | cloud only | env var name the `tts` process reads at call time; must be on the operator's `TTS_PLUGIN_ALLOWED_KEY_ENVS` allowlist. Never pass a literal key. |
| `format` | no | `sherpa`: `wav` (default) \| `pcm`. `openai`: `mp3` (default) \| `wav` \| `pcm`. `elevenlabs`: `mp3` (default) \| `pcm` |
| `speed` | no | `0.25`–`4.0`, default `1.0`, clamped |
| `timeout_ms` | no | default 30000, capped at 60000; cloud hops additionally capped at 30000 by `network` |
| `base_url` | no | override the provider endpoint |
| `model` | no | override the provider model |

### Response

```json
{
  "format": "wav",
  "sample_rate_hz": 24000,
  "num_channels": 1,
  "duration_seconds": 2.4,
  "audio_base64": "UklGR..."
}
```

- `audio_base64` — standard base64 of the audio bytes; decode and write to
  a file (`.wav` / `.mp3` / `.pcm` per `format`) or pipe to a player.
- `sample_rate_hz` / `num_channels` — real for `wav`/`pcm`; `0` for MP3
  bodies (the container carries no header we trust).
- `duration_seconds` — real for `wav`/`pcm`; `0` for MP3.

### Voices per provider

- **sherpa / kokoro** — names from the official table: `af_heart`,
  `af_bella`, `af_nicole`, `af_aoede`, `af_kore`, `af_sarah`, `af_nova`,
  `af_sky`, `am_adam`, `am_echo`, `am_eric`, `am_fenrir`, `am_liam`,
  `am_michael`, `am_onyx`, `am_puck`, `am_santa`, `bf_alice`, `bf_emma`,
  `bf_isabella`, `bf_lily`, `bm_daniel`, `bm_fable`, `bm_george`,
  `bm_lewis`, `ff_siwis`. Escape hatch for custom voice files: `sid:N`.
  Ask the plugin: `tts_voices` with `{"provider":"sherpa"}`.
- **sherpa / piper** — single-speaker: any non-empty `voice` works (maps
  to sid 0). Multi-speaker models: `sid:N`.
- **openai** — `alloy`, `ash`, `ballad`, `coral`, `echo`, `fable`,
  `onyx`, `nova`, `sage`, `shimmer`, `verse`, `amethyst` (validated at
  parse time; unknown → error naming the list).
- **elevenlabs** — any voice id from your account (`21m00Tcm4TlvDq8ikWAM`
  is the classic "Rachel"): list via the ElevenLabs dashboard or
  `GET /v1/voices`.

### Examples

Local Kokoro, WAV out:

```json
{
  "provider": "sherpa",
  "text": "The quick brown fox jumps over the lazy dog.",
  "voice": "af_heart"
}
```

Local Piper, raw PCM:

```json
{
  "provider": "sherpa",
  "text": "Offline, private, fast.",
  "voice": "anything",
  "format": "pcm"
}
```

OpenAI, MP3 out:

```json
{
  "provider": "openai",
  "text": "Hello from the cloud.",
  "voice": "nova",
  "api_key_env": "OPENAI_API_KEY",
  "format": "mp3",
  "model": "gpt-4o-mini-tts"
}
```

ElevenLabs, PCM at 24 kHz:

```json
{
  "provider": "elevenlabs",
  "text": "Hello from ElevenLabs.",
  "voice": "21m00Tcm4TlvDq8ikWAM",
  "api_key_env": "ELEVENLABS_API_KEY",
  "format": "pcm"
}
```

OpenAI-compatible / self-hosted endpoint (any server speaking
`POST /v1/audio/speech`):

```json
{
  "provider": "openai",
  "text": "Local gateway.",
  "voice": "alloy",
  "api_key_env": "OPENAI_API_KEY",
  "base_url": "http://localhost:8880/v1"
}
```

(Pointing `base_url` at loopback requires `network`'s
`NETWORK_PLUGIN_ALLOWED_HOSTS=localhost,127.0.0.1` — see
`plugins/network/config.example.yaml`.)

## `tts_voices`

```json
{ "provider": "sherpa" }
```

Response:

```json
[
  { "id": "af_heart", "name": "af_heart" },
  { "id": "sid:26", "name": "speaker 26" }
]
```

`elevenlabs` → error (voices are per-account).

## Errors

Every failure is `ACTION_ERROR` with a human-readable message in
`ActionResponse.error`. The resolved API key never appears in any error
string.

| Message (contains) | Cause |
|---|---|
| `invalid JSON: ...` | malformed request body |
| `missing required field: provider` | no `provider` |
| `unsupported provider: X` | unknown provider name |
| `missing required field: text` / `text must not be empty` / `text exceeds max length of 4000 chars` | bad `text` |
| `missing required field: voice` / `voice must not be empty` | bad `voice` |
| `unknown openai voice 'X' (known: ...)` | bad openai voice |
| `sherpa supports formats wav\|pcm, got: X` | bad format for provider |
| `missing required field: api_key_env` / `api_key_env must not be empty` | cloud provider without a key reference |
| `api_key_env 'X' is not in the operator's TTS_PLUGIN_ALLOWED_KEY_ENVS allowlist` | key env not allowlisted |
| `environment variable X is not set` | allowlisted env var unset/empty |
| `TTS_PLUGIN_LOCAL_MODEL_DIR is not set ...` | local provider, no model dir configured |
| `TTS_PLUGIN_LOCAL_MODEL_TYPE is not set ...` / `... is unsupported (use 'kokoro' or 'piper')` | local provider, bad model type |
| `missing required model file: <path>` | model dir lacks `model.onnx` / `voices.bin` / `tokens.txt` / `espeak-ng-data` |
| `sherpa-onnx failed to load model from ...` | model dir exists but the files don't form a loadable model |
| `unknown kokoro voice 'X' (known: ...)` | bad kokoro voice name |
| `voice 'X' resolves to sid N, but the model has only M speaker(s)` | voice exists, sid out of range |
| `this piper model has N speakers; use voice "sid:0".."sid:N"` | multi-speaker piper without `sid:` |
| `network plugin call failed: ...` | `network` not registered / IPC error |
| `network plugin error: ...` | `network` returned an action error (SSRF block, timeout, DNS) |
| `provider returned HTTP 4xx/5xx: <body>` | the cloud provider rejected the request |
| `malformed base64 response body: ...` | provider returned broken audio encoding |

## Common patterns

- **Read `tts_voices` first**, cache the list, then synthesize — avoids a
  per-call round trip and surfaces misconfiguration early.
- **Local = private.** `sherpa` never touches the network; the audio never
  leaves the machine. Use it for anything sensitive or high-volume.
- **Cloud = convenience.** `openai`/`elevenlabs` for voices the local
  model can't do. Both normalize to the same response shape, so callers
  can switch providers with a one-field change.
- **WAV for analysis, MP3 for storage.** `wav`/`pcm` responses carry real
  `sample_rate_hz`/`num_channels`/`duration_seconds`; MP3 bodies don't.
- **Synthesize is sequential.** The plugin handles one action at a time
  (same as `network`/`ai`); long local texts block briefly. Keep `text`
  short per call and fan out from your side if you need parallelism.
