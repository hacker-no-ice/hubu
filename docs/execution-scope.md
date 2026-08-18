# Trusted execution scope

Hubu execution-scope schema version 1 replaces the overloaded `merchant`
convention with four independently addressable identities:

- `provider`: the underlying API or service that performs the work.
- `executor`: the trusted execution-plane adapter allowed to claim the token.
- `capability`: the requested outcome, independent of a provider or tool name.
- `billing_merchant`: the party expected to charge the governed account.

Each identity contains a stable `id` and a friendly `display_name`. Requests may
select a trusted catalog entry by stable IDs or unambiguous friendly names.
Hubu resolves all four selectors together and snapshots the canonical catalog
entry in the spend decision. Unknown combinations and combinations that match
more than one catalog entry are rejected. The authorization token references
that immutable decision, and payment or executor validation exact-matches the
complete canonical scope.

```json
{
  "schema_version": 1,
  "provider": "provider:google:gemini-developer",
  "executor": "executor:gongbu:image",
  "capability": "capability:image:generate",
  "billing_merchant": "merchant:google"
}
```

Policies address `provider`, `executor`, `capability`, and `billing_merchant`
directly. A policy value is a stable identity, not a dotted merchant string.
CLI responses show both the friendly name and stable ID for human review.

## Migration from `merchant`

Existing requests and stored decisions remain readable. A request that supplies
only `merchant` is accepted and normalized to a version-1 scope whose provider,
executor, and capability are explicitly `legacy:unresolved`; its billing
merchant receives a deterministic ID derived from the original string. Existing
`merchant` policy rules continue to evaluate the original value.

New callers should send `execution_scope` and omit `merchant`. Supplying both is
rejected so a raw compatibility field cannot contradict or broaden a typed
scope. Legacy rows in the payment-attempt database migrate by adding a nullable
`execution_scope_json` column. New typed attempts persist and restore the exact
canonical scope; old rows remain valid with a null scope.

Gongbu uses the same version-1 JSON schema and catalog fixtures at
`fixtures/execution-scope-v1.json` and `fixtures/execution-scope-catalog-v1.json`,
derives its scope from the operator-selected
provider/adapter target, and sends that canonical scope when claiming Hubu
authorization. It never derives authority from caller-supplied display names.
