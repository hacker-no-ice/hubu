# Local Keychain secrets

Gongbu local-v1 reads provider credentials from the logged-in operator's macOS
Keychain. The provider configuration contains only the Keychain service and
account identifiers; never put the credential in JSON, environment variables,
SQLite, command history, or source control.

Create the item without placing the secret on a command line:

1. Open **Keychain Access** and select the login keychain.
2. Choose **File → New Password Item**.
3. Set **Keychain Item Name** to `gongbu.example`, **Account Name** to `local`,
   enter the provider credential, and save it.

Reference those operator-owned identifiers from the provider configuration:

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

Provider credentials are a separate credential class from the generated
caller-to-Gongbu capability, the Hubu executor/service credential, a request's
spend-auth token ID, and Hubu's human reconciliation capability. Provider
credentials may be replaced in the same Keychain item. Gongbu resolves them at
provider preflight, while caller and Hubu credential changes require restart;
see the [server rotation workflow](server.md#credential-rotation-rollback-and-revocation).
To verify the database contains references rather than plaintext, stop Gongbu
and inspect the schema/data with `sqlite3`; the provider credential itself has
no DB column and must not appear in a `.dump`.
