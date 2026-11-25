# Upgrading to v0.3.0: owner and agent separation

Stable upgrades are an owner action. Codex can no longer administer Hubu.
Owners perform governance changes in the [admin portal](../admin-ui.md) or
through [Hubu CLI commands](../cli.md) that ask the owner to confirm on the
Mac. Each confirmation applies to the action shown in the dialog.

## Before you upgrade

1. Stop sending new spends and let unfinished Gongbu executions finish or
   reconcile them with the current stack.
2. Stop the selected profile with `hubu stack stop`. Back up the Hubu and
   Gongbu databases, profile TOML, and the Codex configuration while the stack
   is stopped. Keep the backup private.
3. Install the pinned v0.3.0 source release using the
   [release operations guide](releases.md). The installer signs `hubu` and
   `hubu-server` ad hoc with the macOS hardened runtime. Keep all four
   production binaries on the same source commit.

## Upgrade a managed profile

1. In the owner's macOS GUI session, run `hubu stack doctor`, then
   `hubu stack start`. If startup stages a generation, review the proposed
   changes and activate the generation as directed before starting again.
2. On opening a v0.2.5 database, Hubu saves a private
   `<database>.pre-v0.3.0-*.sqlite3` snapshot beside the original before making
   schema changes. Successful migration retains ledger postings, budget
   balances, policies and revision history, and registered agents. If the
   schema migration fails, Hubu rolls back the change and leaves the backup
   in place.
3. Open the portal from the updated profile or run an owner command and
   confirm the intended action on the Mac. Touch ID is the default, with a
   macOS login password fallback when biometrics are unavailable. A headless
   Hubu server cannot show the confirmation dialog. Owner actions there
   return `owner_presence_unavailable`.
4. The managed profile issues an agent credential when needed. Stack start
   upgrades a matching Hubu-managed block in the default Codex configuration
   to reference the agent credential. If you created the block in a custom
   file, rerun `hubu init codex --stack-profile /absolute/profile --config /absolute/config.toml`.
   Restart Codex to load the updated block.

The owner bearer still identifies the owner's CLI and portal, and Gongbu
continues to use its separate executor handoff. The v0.2.5 approval and
reconciliation capability files are obsolete and ignored. Doctor reports
them without reading or removing their contents. Keep them during your
rollback window.

## External profiles

In `credentials.toml`, point `files.hubu_agent` at the external Hubu server's
agent credential. The external server needs the corresponding agent token
and the owner's GUI session to present owner confirmations. Existing
`files.hubu_approval` and `files.hubu_reconciliation` entries are accepted,
ignored, and reported by doctor. Refresh any custom Codex configuration with
`hubu init codex` after changing the profile.

## If the upgrade fails

Keep the v0.2.5 backup and resolve the startup error before retrying. To
return to v0.2.5 after a successful schema migration, stop the stack,
restore the pre-upgrade profile and database backups, and reinstall all four
v0.2.5 binaries. Restore the previous Codex configuration only with the
previous stack.
