# Gongbu sandbox

The sandbox is the single startup path for choosing Hubu and provider
boundaries independently. It validates the complete profile before creating
run state or allowing a real provider transmission.

```sh
cargo run -p gongbu-api --bin gongbu-sandbox -- \
  --config examples/sandbox.mock.json
```

The command prints the redacted run manifest and readiness checks. Each run
gets a temporary SQLite path, artifact root, workflow root, log root, and two
loopback-only reserved ports. These are deleted when the command exits. Pass
`--preserve /absolute/new/diagnostics-directory` to retain the manifest and
isolated state for diagnosis; the destination must not already exist.

## Configuration precedence

The JSON profile is loaded first. These narrowly scoped environment overrides
are then applied, and the resulting configuration is validated as one unit:

1. `GONGBU_SANDBOX_HUBU_MODE` (`mock` or `real`)
2. `GONGBU_SANDBOX_PROVIDER_MODE` (`mock` or `real`)
3. `GONGBU_SANDBOX_MAX_SPEND_MINOR`
4. `GONGBU_SANDBOX_LIVE_SPEND_ACK`

Unknown JSON fields, unknown mode values, and mock modes combined with real
endpoint or credential fields fail at startup. A `production` profile rejects
either mock boundary.

## Mode matrix

| Hubu | Provider | Intended use |
| --- | --- | --- |
| Mock | Mock | deterministic local development and ordinary CI |
| Mock | Real | bounded provider validation |
| Real | Mock | Hubu protocol compatibility |
| Real | Real | guarded release dogfood |

The selected implementation is wired through the same `HubuActivities` and
`ProviderActivities` interfaces as the durable production workflow. Modes do
not change persistence, artifact, replay, or settlement invariants.

## Real-boundary gates

Real Hubu requires an explicit `http://` endpoint. The host must be loopback or
appear exactly in `allowlisted_hosts`; it also requires an opaque scoped
credential reference and an isolated non-production account name. Credentials
are references only and never appear in the run manifest.

Real provider mode requires:

- an explicit `target` object containing `workload_type`, `provider`, `adapter`,
  and `model`;
- an existing absolute provider target configuration path;
- an existing absolute frozen pricing catalog path;
- an opaque credential reference;
- a positive `maximum_spend_minor`; and
- the exact acknowledgement `I_ACKNOWLEDGE_LIVE_PROVIDER_SPEND`.

The target configuration must expose exactly one active execution target, it
must match `target`, and fixture adapters are rejected. The credential reference
uses `service:account` and must match that target's Keychain reference. Both
provider files and the referenced Keychain credential are validated before
readiness succeeds. The spend ceiling is enforced again at provider preflight
and invocation against both authorization and the frozen pricing estimate. Mock
provider mode refuses all live-provider fields, cannot spend, and uses a
deterministic one-pixel PNG response.

## Fault injection and replay

Hubu supports `proven_before_mutation` and `commit_then_disconnect` independently
for claim, settle, and release. The mock is stateful: it checks account,
currency, expiry, authorization bounds, claim identity, and finalization state.
Repeated operation keys return the existing claim or settlement without another
financial mutation. Its call log stores only a SHA-256 operation-key digest.

Provider `scenario` supports `success`, `proven_rejection`,
`timeout_ambiguous`, and `malformed_response`. `execution_fault` additionally
supports the same proven-before-mutation and commit-then-disconnect distinction.
The provider caches outcomes by durable attempt ID, proving replay does not
perform a second invocation.

Artifact-persistence faults belong at the existing `ArtifactActivities`
boundary rather than masquerading as provider responses. The sandbox wraps that
activity with the configured `artifact_fault` while continuing to use the same
durable workflow.

## Diagnostics and secrets

The manifest contains modes, seed, redacted endpoint, file digests, process
version, optional build commit, isolated paths and ports, spend ceiling, and
readiness results. Query strings are removed from endpoints. Raw credentials,
credential references, authorization tokens, prompts, account identifiers, and
provider responses are not serialized to the manifest or safe mock call log.
