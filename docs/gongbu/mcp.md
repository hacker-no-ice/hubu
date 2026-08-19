# Standalone Gongbu MCP compatibility server

`gongbu-mcp` is a local stdio MCP adapter over Gongbu's authenticated v1 HTTP
API. It does not import Gongbu persistence, providers, pricing, workflows, or
artifact storage. Replay and immutable-scope conflict behavior therefore remain
identical to direct HTTP calls.

The recommended agent setup uses `hubu-unified-mcp` and configures Gongbu in the
same client entry as Hubu. Use this standalone adapter only for explicit
compatibility or rollback during the migration window; see the
[unified MCP migration guide](../unified-mcp-migration.md).

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

- `gongbu_create_execution` mirrors canonical `POST /v2/executions`. Retrying a client
  call with the same `spend_auth_token_id` and execution intent relies on
  Gongbu's token-to-authoritative-operation replay guarantee; the adapter itself
  never retries the POST. Money, scope, account, operation key, task ID, and
  reason come from Hubu or Gongbu's operator catalog, not MCP arguments.
- `gongbu_get_execution` takes `execution_id`.
- `gongbu_list_artifacts` takes `execution_id`.
- `gongbu_get_artifact` takes `artifact_id` and returns safe metadata plus an MCP
  image content block containing base64 bytes. It never returns a storage key or
  filesystem path.

Example `tools/call` request (one JSON object per stdio line):

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"gongbu_create_execution","arguments":{"schema_version":2,"spend_auth_token_id":"00000000-0000-4000-8000-000000000123","input":{"prompt":"A small blue circle","image_count":1},"input_schema_version":1,"workload_type":"image_generation","provider":"google","adapter":"gemini_image","model":"gemini-2.5-flash-image"}}}
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
