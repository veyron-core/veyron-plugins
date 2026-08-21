# search plugin

Web search for vynkor plugins. Exposes one action, `web_search`. Doesn't
open its own sockets — every request is routed through the `network`
plugin's `http_request` action, so `network` must also be registered and
running (same model as `ai`/`tts`/`stt`). See `ROADMAP.md` for the design
rationale.

v1 supports two providers: `brave` (Brave Search API) and `tavily` (Tavily
Search API), behind one normalized interface.

## Operator note

`search` declares two kernel permissions — `network` and `secrets`
(`plugin.json`: `"permissions": ["network", "secrets"]`) — because it
invokes the `network` plugin's gated `http_request` action and the
`secrets` plugin's gated `secret_get` action, and the kernel's
anti-laundering check (T-19) requires callers of a gated action to hold
its permission too (Manifest v2: per-action `permission` on
`http_request` and `secret_get`). It opens no sockets itself, so it's safe
to run with `sandbox: true`. `network` still needs `sandbox: false` (real
egress) — see `plugins/network/README.md`.

## Action: `web_search`

Request (`ActionRequest.params_json`):

```json
{
  "query": "vynkor plugin kernel",
  "provider": "brave",
  "api_key_env": "SEARCH_BRAVE_KEY",
  "count": 5,
  "timeout_ms": 30000
}
```

- `query` — required, non-empty, capped at 400 chars.
- `provider` — `"brave"` (default) or `"tavily"`.
- `api_key_env` — required. Name under which the `search` process resolves
  the key at call time, never a literal key. The caller never puts the raw
  key in the payload. Resolution is vault-first: `search` asks the
  `secrets` plugin's vault for a secret stored under that exact name
  (`secret_set {"name":"...","value":"..."}` by the operator), and falls
  back to the environment variable of the same name only when the vault
  has no non-empty value. The vault wins when both exist. Must appear in
  the operator's `SEARCH_PLUGIN_ALLOWED_KEY_ENVS` allowlist (see
  "Configuration") — otherwise a caller could name *any* secret/env var
  the process has, not just a provider key, and exfiltrate it via a
  caller-controlled `base_url`. Not allowlisted, or unset in both sources
  → `ACTION_ERROR`; the key value never appears in any error string.
- `base_url` — optional per-provider override (defaults:
  `https://api.search.brave.com`, `https://api.tavily.com`).
- `count` — optional, default `5`, capped at `20`.
- `timeout_ms` — optional, default and cap `30000`.

Response (`ActionResponse.data_json`) on success, normalized across both
providers:

```json
{
  "query": "vynkor plugin kernel",
  "results": [
    { "title": "vynkor", "url": "https://example.com/vynkor", "snippet": "A plugin kernel" }
  ]
}
```

`results` may be empty (a query can legitimately return no hits). Snippet
sources: `description` for brave, `content` for tavily.

Errors → `ACTION_ERROR` with a human-readable message: malformed/missing
request fields, `api_key_env` not on the operator's allowlist or unset,
malformed provider JSON, non-2xx HTTP status from the provider, or any
error `network`'s `http_request` itself returns (SSRF block, timeout, DNS
failure, connection refused).

## Configuration

`search` reads no config file itself. The only configuration is environment
variables set in the kernel's `config.yaml`, under this plugin's `env:`
list — see `config.example.yaml` in this directory. Provider keys are
resolved vault-first: at call time `search` asks the `secrets` plugin's
vault for the key under the `api_key_env` name, and only falls back to the
plugin's own environment variables when the vault has no non-empty value.
The vault wins when both exist — so the operator may store keys in the
vault instead of `env:` (via `secret_set
{"name":"SEARCH_BRAVE_KEY","value":"..."}`, requires the `secrets` plugin
to be registered), or keep using `env:` as before.

`SEARCH_PLUGIN_ALLOWED_KEY_ENVS` is **required**: a comma-separated,
exact-match allowlist of every env var name a caller's `api_key_env` may
reference. Default-deny — omit it and every `web_search` request is
rejected. Without this allowlist a caller could set `api_key_env` to any
env var the `search` process happens to have (an unrelated secret, not
just a provider key) and have its value sent straight into an outbound
request header to a `base_url` the caller also controls.

```yaml
plugins:
  - id: search
    binary: /opt/plugins/search
    sandbox: true
    env:
      - SEARCH_PLUGIN_ALLOWED_KEY_ENVS=SEARCH_BRAVE_KEY,SEARCH_TAVILY_KEY
      - SEARCH_BRAVE_KEY=<brave subscription token>
      - SEARCH_TAVILY_KEY=tvly-...
```

## Testing

`cargo test` — no live network. Provider adapters are tested against
fixture JSON (happy, malformed, missing-field, empty-results), request
parsing and the allowlist are unit-tested, and a fake-kernel integration
test drives the full handler end-to-end over `UnixStream::pair` (a shim
answers `PluginRegister`, `secret_get`, and `http_request`), asserting the
normalized output, that the auth header carries the vault-resolved key
(vault wins over env), and that no error path leaks the key value.
`network`'s own tests cover the actual HTTP send.
