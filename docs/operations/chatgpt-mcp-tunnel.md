# Use local Hubu tools in ChatGPT

Connect a local Hubu stack to ChatGPT with Secure MCP Tunnel. Use the configured
MCP tools in Chat or Work, including on mobile where your account supports the
connection, without opening an inbound port on your Mac or using Codex Remote.

This runbook covers macOS with zsh. The setup was exercised with
`tunnel-client v0.0.14` and Hubu `v0.2.1-rc.3`: discovery and backend health
worked, and adding the operation registry restored billable-tool availability.
Paid execution and protected approvals through ChatGPT were not validated.
Tool availability is not evidence that execution succeeded.

## How it works

```text
ChatGPT Chat / Work
        |
OpenAI-hosted tunnel
        |
outbound HTTPS from your Mac
        |
tunnel-client
        |
stdio launcher → hubu-unified-mcp
                      /     \
                 Hubu API  Gongbu API
```

The tunnel ID and access settings persist on OpenAI. The client on your Mac
long-polls for MCP requests, forwards them locally, and returns responses.
The live connection requires the client to keep running. This exposes the
configured MCP tools, not general access to your computer.

If Codex already runs unified MCP, its stdin/stdout pipes belong to Codex. The
tunnel client needs its own adapter process and pipes. Both adapters call the
existing Hubu and Gongbu backends; no second backend stack is needed. Unified
MCP does not need HTTP support for this stdio connection.

## Prerequisites

- Install a matching Hubu release and start a selected profile using the
  [local stack guide](../local-stack.md).
- Locate the unified MCP executable, backend endpoints, and separate bearer
  credential-file paths. An existing Codex setup records these under
  `[mcp_servers.hubu]` and `[mcp_servers.hubu.env]` in its managed configuration.
  Inspect locally; do not paste configuration containing credentials into chat.
- Create a tunnel in [Platform tunnel settings](https://platform.openai.com/settings/organization/tunnels)
  and record its `tunnel_...` ID. Associate it with the intended ChatGPT
  workspace as well as the appropriate Platform organization.
- Have ChatGPT developer-mode access. This is separate from Platform tunnel
  permissions and depends on account/workspace policy.

Keep this setup within your trusted personal/local environment. The launcher
uses local backend credentials; it does not establish a separate Hubu identity
for each ChatGPT user. A workspace association does not replace that boundary.
See the [approval contract](../unified-mcp.md#approval-boundary) before sharing
access with others.

## Install tunnel-client

Download from Platform tunnel settings or the
[official releases](https://github.com/openai/tunnel-client/releases/latest).
Check the Mac architecture:

```sh
uname -m
```

Choose the standard `tunnel-client-<version>-darwin-arm64.zip` for Apple Silicon
(`arm64`) or `darwin-amd64.zip` for Intel (`x86_64`). The tested Apple Silicon
archive was `tunnel-client-v0.0.14-darwin-arm64.zip`. Check its SHA-256 against
the release checksum manifest:

```sh
shasum -a 256 "$HOME/Downloads/tunnel-client-v0.0.14-darwin-arm64.zip"
```

Extract the archive, then install it. Adjust the source path for your download:

```sh
mkdir -p "$HOME/.local/bin"
install -m 755 \
  "$HOME/Downloads/tunnel-client-v0.0.14-darwin-arm64/tunnel-client" \
  "$HOME/.local/bin/tunnel-client"
export PATH="$HOME/.local/bin:$PATH"
tunnel-client help quickstart
```

Add the PATH export to `~/.zshrc` once if needed in future terminals. If macOS
blocks the binary, verify its source/checksum and use the macOS Privacy &
Security approval flow for that binary.

## Load the runtime API key

Create a regular key in [Platform API keys](https://platform.openai.com/settings/organization/api-keys)
in the tunnel's organization. Name it something like `hubu-tunnel-mac`.
The documented restricted-key selection is **Tunnels Read + Use**. The key's
principal also needs these organization-level permissions. The daemon does not
need Tunnels Manage or an Admin API key. If Tunnels is absent in your permission
list, resolve the account/UI mismatch rather than guessing model permissions.
See the [client permission guide](https://github.com/openai/tunnel-client/blob/v0.0.14/docs/permissions.md#creating-keys).

In your Mac's zsh Terminal, enter the key at a hidden prompt:

```sh
read -r -s "CONTROL_PLANE_API_KEY?Paste your OpenAI runtime API key: "
printf '\n'
export CONTROL_PLANE_API_KEY
```

Before `?` is the destination variable; after it is the prompt. `-s` hides input
and `-r` preserves backslashes. Capitalization is only a naming convention.
The input is not entered into shell command history. `export` makes the variable
available to child processes in this terminal, not other terminals. These
commands do not save the key to a file. Do not put it in chat, scripts, or Git.

## Create the stdio launcher

Create a private state directory and edit a new launcher:

```sh
mkdir -p "$HOME/.local/bin" "$HOME/.local/state/hubu-chatgpt"
chmod 700 "$HOME/.local/state/hubu-chatgpt"
nano "$HOME/.local/bin/hubu-tunnel-stdio"
```

Paste this script. Replace every `/absolute/path/...` with your installation's
paths and change the example endpoints if your profile uses different addresses:

```sh
#!/bin/sh
set -eu
umask 077

export HUBU_UNIFIED_HUBU_ENDPOINT="http://127.0.0.1:8787"
export HUBU_UNIFIED_HUBU_BEARER_TOKEN_FILE="/absolute/path/to/hubu/auth"
export HUBU_UNIFIED_GONGBU_ENDPOINT="http://127.0.0.1:8788"
export HUBU_UNIFIED_GONGBU_BEARER_TOKEN_FILE="/absolute/path/to/gongbu/caller"
export HUBU_UNIFIED_OPERATION_STATE_PATH="$HOME/.local/state/hubu-chatgpt/operations.sqlite3"

# Do not trust unverified client human-approval gates during initial setup.
export HUBU_MCP_TRUST_CLIENT_APPROVAL=0
export HUBU_MCP_TRUST_SPEND_APPROVAL=0
unset HUBU_APPROVAL_TOKEN HUBU_APPROVAL_TOKEN_FILE
unset HUBU_RECONCILIATION_TOKEN HUBU_RECONCILIATION_TOKEN_FILE

# Use these credential files and the router's default operation-key allocation.
unset HUBU_UNIFIED_HUBU_BEARER_TOKEN HUBU_UNIFIED_GONGBU_BEARER_TOKEN
unset HUBU_UNIFIED_OPERATION_KEY_DB

# The parent tunnel client keeps the OpenAI key; Hubu does not need it.
unset CONTROL_PLANE_API_KEY OPENAI_ADMIN_KEY
exec /absolute/path/to/hubu-unified-mcp
```

Save with Control-O, Return, then Control-X. Check syntax and permissions:

```sh
sh -n "$HOME/.local/bin/hubu-tunnel-stdio"
chmod 700 "$HOME/.local/bin/hubu-tunnel-stdio"
```

The wrapper bundles configuration for the new adapter. `exec` replaces it with
unified MCP. Keep stdout reserved for MCP; send diagnostics to stderr.
Unsetting the OpenAI key only affects this child process: the parent Terminal
and tunnel client retain it.

**The operation registry is required.** Without an absolute writable
`HUBU_UNIFIED_OPERATION_STATE_PATH`, health checks can work while spending and
execution tools are hidden with `configuration_missing`. Use a persistent
registry dedicated to this integration. Preserve it across restarts/upgrades;
deleting it loses operation identity and recovery state. This is adapter state,
separate from the backend databases.

The initial flags disable protected approval/admin actions, not all billable
operations. Do not submit spending as a connectivity test. Registry availability
also does not prove the client supplies the required
[trusted invocation metadata](../unified-mcp.md#trusted-invocation-metadata).

## Create and check the profile

In the terminal holding your exported key, replace `tunnel_REPLACE_ME` with your
tunnel ID. Initialize this new profile once:

```sh
tunnel-client init \
  --sample sample_mcp_stdio_local \
  --profile hubu-stdio \
  --tunnel-id tunnel_REPLACE_ME \
  --mcp-command "$HOME/.local/bin/hubu-tunnel-stdio" \
  --health-listen-addr 127.0.0.1:0

tunnel-client doctor --profile hubu-stdio --explain
```

The generated profile stores `env:CONTROL_PLANE_API_KEY`, not the literal key.
`hubu-stdio` is a tunnel-client profile, distinct from the Hubu stack profile.
Port `0` selects an available loopback port for health/admin UI. Use the address
reported at startup; do not expose the admin listener publicly.

Confirm that the existing selected stack is ready:

```sh
hubu stack status
```

If stopped, run `hubu stack start`. If no profile is selected, follow the
[profile selection steps](../local-stack.md) first. Resolve diagnostics before
continuing; starting a process does not establish readiness.

## Run and keep the Mac awake

```sh
caffeinate -i tunnel-client run --profile hubu-stdio
```

Keep this terminal open. `caffeinate -i` prevents idle system sleep while the
client runs, keeping the whole Mac available, including Hubu and Gongbu. The
screen can turn off. Closing the laptop lid or explicitly sleeping the Mac can
still interrupt service. A second caffeinate process for the stack is not
needed while this one runs.

Run without `caffeinate -i` when sleep prevention is unnecessary. Control-C
stops this foreground connection without deleting the tunnel or stopping the
separate backends. For later managed operation, consult
`tunnel-client runtimes --help`; this guide uses a visible foreground process.

## Connect and verify in ChatGPT

With the client running:

1. On ChatGPT web, enable **Settings → Security and login → Developer mode**
   where permitted.
2. Open [Plugins](https://chatgpt.com/plugins), select **+**, and create a
   connection named **Hubu**.
3. Choose **Connection → Tunnel**, then select your tunnel or enter its ID.
4. Create the connection and inspect the discovered tools.
5. Start a new **Chat or Work** conversation and select Hubu. On mobile, use
   the same account/workspace where that custom connection is available.

Use this first prompt:

> Call hubu_health and hubu_unified_capabilities. Report backend availability
> and operation_registry state. Do not change configuration or submit spending.

Look for both backends to be available and this capability object:

```json
{
  "operation_registry": {
    "state": "available",
    "reason_code": null,
    "billable_operations_available": true
  }
}
```

Inspect actual tool results, not only the assistant's summary. Advertised tools
may still reject calls because of approval or identity requirements. Before
enabling protected actions, verify the client's human prompts against the
[approval contract](../unified-mcp.md#approval-boundary). Do not set trust flags
just to remove an error. Any later paid test needs explicit authorization and a
budget; this runbook stops at connectivity and capability checks.

For an optional icon, the tested setup UI accepts PNG, recommends at least
256 × 256, and limits files to 10 KB. Center the full wordmark on a padded square
canvas to avoid cropping. A palette PNG with the site's `#050505` background
fits well. Save a new file and preserve the source SVG.

## Restart and refresh tools

Launcher environment changes apply to new processes:

1. Press Control-C in the tunnel terminal.
2. Restart with `caffeinate -i tunnel-client run --profile hubu-stdio`.
3. Open Hubu's connection details in ChatGPT Plugins and select **Refresh**.
   Reloading the browser page alone does not refresh MCP metadata.
4. Start a new conversation with Hubu and repeat the read-only capability check.

Do not delete the registry or create a replacement tunnel to refresh tools.
In a new terminal, load/export the key again before restarting. After stopping
the client, `unset CONTROL_PLANE_API_KEY` clears it from that shell; it does not
revoke the key in Platform.

## Troubleshooting

| Symptom | Check or fix |
| --- | --- |
| Command not found | Confirm the executable directory is on this terminal's PATH. |
| Runtime authentication fails | Check key organization, restricted permissions, and the principal's Tunnels Read + Use. Do not print the key. |
| Tunnel absent in ChatGPT | Check its ChatGPT workspace association and operator permissions, not only the Platform organization. |
| Calls time out | Check Mac sleep, network, tunnel client, and stack readiness; run `doctor --explain`. |
| Health works but spending tools are missing | Inspect `operation_registry`: `configuration_missing` needs the persistent path; `state_unavailable` needs path/permission diagnosis. Restart and refresh afterward. |
| Protected actions are rejected | Initial trust flags are off and approval/reconciliation credentials are withheld. Verify the client approval contract before changing them. |
| Spending fails on identity metadata | Transport alone does not supply trusted harness identity. Diagnose the exact error against unified MCP; do not put private operation keys in model arguments. |
| A launcher change seems ineffective | Restart the tunnel-owned MCP process, refresh the connection's metadata, and start a new conversation. |
| Icon crops the wordmark | Export a padded square PNG under the upload limit. |

## References

- [Secure MCP Tunnel](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels)
- [Connect and refresh plugins](https://developers.openai.com/plugins/deploy/connect-chatgpt)
- [Chat and Work plugin support](https://learn.chatgpt.com/docs/plugins)
- [Tunnel client permission guide](https://github.com/openai/tunnel-client/blob/v0.0.14/docs/permissions.md)
- [Unified MCP](../unified-mcp.md)
