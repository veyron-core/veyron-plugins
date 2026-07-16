# ai plugin

Provider-agnostic chat completion for Veyron plugins. Exposes one action,
`chat_completion`. Doesn't open its own sockets — every request is routed
through the `network` plugin's `http_request` action, so `network` must
also be registered and running. See `ROADMAP.md` for the full design
rationale ("Decision: reuse `network`, don't reinvent").

v1 supports two providers: `anthropic` (Claude Messages API) and `openai`
(OpenAI-compatible chat completions — covers OpenAI, OpenRouter, and local
Ollama, since all three speak the same wire shape).

## Operator note

`ai` declares zero kernel permissions (`plugin.json`: `"permissions": []`)
and opens no sockets itself, so it's safe to run with `sandbox: true`.
`network` still needs `sandbox: false` (real egress) — see
`plugins/network/README.md`.

## Action: `chat_completion`

Request (`ActionRequest.params_json`):

```json
{
  "provider": "anthropic",
  "base_url": "https://api.anthropic.com",
  "model": "claude-sonnet-5",
  "api_key_env": "ANTHROPIC_API_KEY",
  "messages": [{"role": "user", "content": "..."}],
  "max_tokens": 1024,
  "timeout_ms": 30000
}
```

- `provider` — `"anthropic"` or `"openai"`. Required.
- `base_url` — required for `openai` (no safe default across
  OpenAI/OpenRouter/Ollama/self-hosted). Optional for `anthropic`, defaults
  to `https://api.anthropic.com`.
- `model` — required, non-empty.
- `api_key_env` — name of an environment variable the `ai` **process**
  reads at call time, not a literal key. The caller never puts the raw key
  in the payload. Must appear in the operator's `AI_PLUGIN_ALLOWED_KEY_ENVS`
  allowlist (see "Configuration") — otherwise a caller could name *any* env
  var the process has, not just a provider key, and exfiltrate it via a
  caller-controlled `base_url`. Not allowlisted, or unset → `ACTION_ERROR`;
  the key value never appears in any error string.
- `messages` — required, non-empty, `{role, content}` pairs.
- `max_tokens` — optional, default `1024`, capped at `8192`.
- `timeout_ms` — optional, default and cap `30000`.

Response (`ActionResponse.data_json`) on success, normalized across both
providers:

```json
{ "content": "...", "stop_reason": "end_turn", "usage": {"input_tokens": 1, "output_tokens": 2} }
```

Errors → `ACTION_ERROR` with a human-readable message: malformed/missing
request fields, `api_key_env` not on the operator's allowlist or unset,
malformed provider JSON, non-2xx HTTP status from the provider, or any
error `network`'s `http_request` itself returns (SSRF block, timeout, DNS
failure, connection refused).

## Configuration

`ai` reads no config file itself. The only configuration is environment
variables set in the kernel's `config.yaml`, under this plugin's `env:`
list — see `config.example.yaml` in this directory.

`AI_PLUGIN_ALLOWED_KEY_ENVS` is **required**: a comma-separated,
exact-match allowlist of every env var name a caller's `api_key_env` may
reference. Default-deny — omit it and every `chat_completion` request is
rejected. Without this allowlist a caller could set `api_key_env` to any
env var the `ai` process happens to have (an unrelated secret, not just a
provider key) and have its value sent straight into an outbound request
header to a `base_url` the caller also controls.

```yaml
plugins:
  - id: ai
    binary: /opt/plugins/ai
    sandbox: true
    env:
      - AI_PLUGIN_ALLOWED_KEY_ENVS=ANTHROPIC_API_KEY,OPENAI_API_KEY
      - ANTHROPIC_API_KEY=sk-ant-...
      - OPENAI_API_KEY=sk-...
```

## Talking to a local model (Ollama)

Point `base_url` at Ollama's OpenAI-compatible endpoint and use `provider:
"openai"`:

```json
{
  "provider": "openai",
  "base_url": "http://localhost:11434/v1",
  "model": "deepseek-coder:1.3b",
  "api_key_env": "OLLAMA_API_KEY",
  "messages": [{"role": "user", "content": "hi"}]
}
```

Ollama needs no auth — pick any unset/empty variable name for
`api_key_env`, add it to `AI_PLUGIN_ALLOWED_KEY_ENVS` like any other (still
required even though the value itself is empty), and leave the var unset
or empty in `env:`; the `openai` adapter omits the `Authorization` header
entirely when the resolved key is empty.

This also requires `network`'s own config: its built-in SSRF blocklist
blocks loopback by default, so its `env:` needs

```yaml
- NETWORK_PLUGIN_ALLOWED_HOSTS=localhost,127.0.0.1
```

(see `plugins/network/config.example.yaml`) or every request to a local
model fails.

## Testing

`cargo test` — 24 unit tests, no live network (provider adapters are
tested against fixture JSON; `network`'s own tests cover the actual HTTP
send). End-to-end behavior (this README's examples, plus the SSRF
limitation above) was verified against a real kernel + `network` + `ai` +
local Ollama stack; there's no automated integration test for that yet.
