# Gongbu MCP server

`gongbu-mcp` is a local stdio MCP adapter over Gongbu's authenticated v1 HTTP
API. It does not import Gongbu persistence, providers, pricing, workflows, or
artifact storage. Replay and immutable-scope conflict behavior therefore remain
identical to direct HTTP calls.

## Operator configuration

Set these in the MCP server process environment, not in tool arguments:

- `GONGBU_MCP_ENDPOINT` (required): Gongbu HTTP base URL, such as
  `http://127.0.0.1:8787/`.
- `GONGBU_MCP_BEARER_TOKEN` (required): operator-issued credential whose Gongbu
  authentication claim fixes the account identity.
- `GONGBU_MCP_CONNECT_TIMEOUT_MS` (optional, default `2000`).
- `GONGBU_MCP_REQUEST_TIMEOUT_MS` (optional, default `30000`).

Timeouts must be 1–300000 ms. Redirects and automatic retries are disabled.
The endpoint, credential, account identity, headers, pricing, artifact roots,
deadlines, and retry behavior cannot be supplied or overridden by tool input.

Run it with:

```sh
GONGBU_MCP_ENDPOINT=http://127.0.0.1:8787 \
GONGBU_MCP_BEARER_TOKEN=operator-issued-token \
cargo run -p gongbu-mcp
```

Configure an MCP client with command `cargo`, arguments
`["run", "--quiet", "-p", "gongbu-mcp"]`, the repository as its working
directory, and the two required environment variables above.

## Tools

- `gongbu_create_execution` mirrors `POST /v1/executions`. Retrying a client
  call with the same operation scope relies on Gongbu's `(account_id,
  operation_key)` replay guarantee; the adapter itself never retries the POST.
- `gongbu_get_execution` takes `execution_id`.
- `gongbu_list_artifacts` takes `execution_id`.
- `gongbu_get_artifact` takes `artifact_id` and returns safe metadata plus an MCP
  image content block containing base64 bytes. It never returns a storage key or
  filesystem path.

Example `tools/call` request (one JSON object per stdio line):

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"gongbu_create_execution","arguments":{"schema_version":1,"operation_key":"agent-op-001","hubu_authorization_id":"auth-001","hubu_claim_id":null,"hubu_token_reference":"operator-stored-ref","authorization":{"amount_minor":25,"currency":"USD"},"input":{"prompt":"A small blue circle","image_count":1},"input_schema_version":1,"workload_type":"image_generation","provider":"google","adapter":"gemini_image","model":"gemini-2.5-flash-image"}}}
```

Example artifact listing:

```json
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"gongbu_list_artifacts","arguments":{"execution_id":"execution-id"}}}
```

## Tests

Mocked HTTP tests are deterministic and make no Hubu or provider calls:

```sh
cargo test -p gongbu-mcp
```

To opt into the real-local-service smoke test, provide an existing execution
owned by the configured token:

```sh
GONGBU_MCP_INTEGRATION=1 \
GONGBU_MCP_ENDPOINT=http://127.0.0.1:8787 \
GONGBU_MCP_BEARER_TOKEN=operator-issued-token \
GONGBU_MCP_INTEGRATION_EXECUTION_ID=execution-id \
cargo test -p gongbu-mcp opt_in_real_gongbu_execution_read -- --nocapture
```
