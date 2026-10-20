# Upgrading to v0.2.5: provider packs

v0.2.5 runs the shipped Gemini image targets from a verified
[provider pack](../provider-packs.md) instead of a dedicated Rust adapter.
Contract IDs, `provider_config_version` values, pricing versions, prices and
provider behavior are unchanged. What changes is each target's settings, which
now name the pack and its content digest, so the targets' revision digests
change.

An execution freezes the digest of the target revision it was created with.
Unfinished work frozen against the old digest can't resume or be reconciled
after the upgrade, so **drain unfinished work before you upgrade**. Gongbu
refuses to start until you do.

This includes unfinished rows migrated from older databases without a known
target digest (`legacy-unresolved`). A matching target name and version alone
cannot prove the original revision. Finish or reconcile those rows with the
previous version and configuration before upgrading; completed legacy rows
do not block startup.

## Before you upgrade

1. Stop sending new governed provider work to the stack.
2. Let every Gongbu execution finish. Resolve anything waiting for
   reconciliation on the current version: reconciliation-pending work counts
   as unfinished.
3. Stop the stack: `hubu stack stop`.
4. Take cold backups of the Hubu and Gongbu state directories.

## Upgrade

1. Install v0.2.5.
2. Re-render and activate the stack so the provider target config carries the
   new pack settings: `hubu stack render`, review, then activate. A provider
   target config rendered by an earlier version names the removed
   `gemini_developer_image` settings type and is rejected at startup.
3. Start the stack.

## If Gongbu refuses to start

Gongbu checks every unfinished execution against the active provider
configuration at startup. If any references a target revision that no longer
exists, it stops with a message such as:

```text
1 unfinished execution(s) reference provider target revisions this configuration no longer has (for example image_generation/google/gemini_developer_image/gemini-3.1-flash-image); finish or reconcile them with the previous Gongbu version and provider configuration, then upgrade
```

Reinstall the previous version and its rendered provider configuration, let
the listed work finish or reconcile it, then upgrade again. The same check
applies to any later change that replaces a provider target revision.

## What is no longer available

Redaction attestation re-reads the target revision of a finished FLUX
execution. Once FLUX moves to a provider pack, attestation of FLUX executions
created before that upgrade fails with `UnsupportedTarget`. Receipts, the
ledger, settlement history and artifact retrieval are unaffected.
