# Local Keychain secrets

Gongbu local-v1 reads provider credentials from the logged-in operator's macOS
Keychain. The provider configuration contains only the Keychain service and
account identifiers; never put the credential in JSON, environment variables,
SQLite, command history, or source control.

Create or replace a generic-password item without placing the secret on the
command line:

```sh
security add-generic-password -U -s gongbu.example -a local -w
```

The command prompts securely for the value. Reference it from the operator-owned
provider configuration:

```json
{
  "provider_configs": [{
    "provider_config_version": "example-image-2026-08-05",
    "workload_type": "image_generation",
    "provider": "example",
    "adapter": "fixture",
    "model": "image-v1",
    "secret_service": "gongbu.example",
    "secret_account": "local",
    "enabled": true
  }]
}
```

Set `GONGBU_PROVIDER_CONFIG` to that file and restart Gongbu. At startup/execution
preflight, a missing item or denied Keychain access fails as
`provider error (secret_unavailable)` before claim or provider work. Gongbu does
not include Keychain stderr in the response.

To replace a credential manually, rerun the `security add-generic-password -U`
command, then restart Gongbu so subsequent executions use the replacement. To
verify the database contains references rather than plaintext, stop Gongbu and
inspect the schema/data with `sqlite3`; the provider credential itself has no DB
column and must not appear in a `.dump`.
