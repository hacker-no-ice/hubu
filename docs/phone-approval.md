# Private phone approval

Hubu can optionally publish actionable approval notifications to a self-hosted
ntfy server. Approve and Deny record the human decision in Hubu. Approval reserves
budget; it never calls Gongbu or a provider. Resume through
`hubu_resume_operation` with the original public operation handle. Denial creates
neither a reservation nor spend.

## Configure an isolated rehearsal

Do not change a running profile without reviewing its configuration first. Keep
the normal Hubu API on its existing loopback address. Phone mode adds a separate
**callback-only listener** on the exact private address in the callback URL;
agent API routes are unavailable on this listener.

Set these environment variables in the Hubu server process only:

| Variable | Value |
| --- | --- |
| `HUBU_APPROVAL_NTFY_URL` | Private ntfy server root, e.g. `http://192.168.1.20:8080/` |
| `HUBU_APPROVAL_NTFY_TOPIC` | An access-controlled subscription topic |
| `HUBU_APPROVAL_CALLBACK_URL` | e.g. `http://192.168.1.10:8788/spend/approval/phone` |
| `HUBU_APPROVAL_NTFY_TOKEN` | Optional ntfy publisher bearer credential, kept outside the harness |

The URLs require literal IPv4 addresses: loopback, RFC1918 LAN addresses, or the
private Tailscale address range. DNS names, wildcard binds, public IPs, redirects,
URL credentials, queries and fragments are rejected. Loopback is useful for mock
tests only; a phone needs the LAN/private-network address. Never port-forward,
reverse-proxy publicly, or tunnel this callback to the public internet. Restrict
firewall access to the intended phone/private peers. HTTP on a trusted LAN does
not encrypt tokens; prefer an encrypted private network such as Tailscale. The
built-in listener speaks HTTP and rejects HTTPS callback URLs; use Tailscale for
encrypted transport. ntfy publishing can use private HTTPS with a valid certificate.

Configure ntfy topic access controls so only the owner can subscribe/read and the
Hubu server can publish. The message and action bodies contain sensitive approval
capabilities; do not give the agent the subscription or publishing credentials.
Ordinary API responses never return these action capabilities. Existing Hubu
approval-key files must remain outside the agent's accessible runtime.

Phone mode disables `POST /spend/approval/resolve` even when an older harness has
`--trust-client-approval` and the approval capability. Do not rely on agent
self-approval in this flow. Ordinary policy-allowed operations are unaffected.

## Display and delivery

Notifications show the registered agent name, requested dollar amount, and trusted
provider/capability display names from the resolved execution scope. They omit
topics, tokens, internal IDs, and arbitrary request reasons from visible text.
Image dimensions/model are not part of the current Hubu approval snapshot, so
this implementation does **not** invent a `2k` size label. The exact image scope
remains in the immutable operation intent; adding trusted dimensions is follow-up
contract work.

The native phone app uses ntfy HTTP actions with a JSON body; it does not need a
Hubu API bearer token. See [ntfy action documentation](https://docs.ntfy.sh/publish/#action-buttons)
and [iOS release notes](https://docs.ntfy.sh/releases/#ntfy-ios-app-v11), which
confirm HTTP actions in the notification and detail view. Device behavior must
still be rehearsed. Self-hosted iPhone instant delivery needs the ntfy
[iOS push setup](https://docs.ntfy.sh/config/#ios-instant-notifications); upstream
poll wake-ups do not authorize public exposure of Hubu. Phone subscription,
platform push setup and private-network reachability are external setup tasks.
No physical phone acceptance or live provider settlement is claimed by mock tests.

Each token is HMAC-SHA256 signed with the separate Hubu approval authority and
bound to one immutable decision and a ten-minute expiration from its creation.
Approve/Deny compete for one durable final decision. A reused or expired token is
rejected, including after server restart. The phone receives only confirmation,
never an executor authorization token. Request origins/proxy headers and
non-private peers are rejected.

Publishing is bounded to two seconds, outside governance locks. Successful
notifications are deduplicated in memory until expiry. Restart may redeliver the
same still-valid token; consumption stays durable. Failure leaves approval pending;
replaying the **exact same** authorization request retries delivery. There is no
background delivery queue. Expired phone actions require the owner fallback below;
do not create a replacement spend request.

## Continue and recover

While waiting, read the original operation status. After approval call
`hubu_resume_operation` with that same handle; repeated calls continue or observe
the same immutable intent. A phone tap alone does not automatically start provider
work. If Codex ends its turn, send a short follow-up: “Resume the existing approved
operation using its original handle; do not create a new request.” Whether a real
Codex turn stays alive long enough remains a rehearsal question.

If push delivery fails or the phone action expires, stop the Hubu server, remove
`HUBU_APPROVAL_NTFY_URL` from its launch environment, and restart the **same**
profile/database with the **same** approval authority. The original pending
approval persists. Use the owner terminal's existing approval command/capability
to approve or deny that decision, then resume the original handle. Do not mount
the human authority into the agent. This explicit restart is needed because phone
mode intentionally disables the old approval mutation route.

Before recording, verify private reachability, notification presentation, both
buttons, exactly one reservation, same-handle resume and settlement, and denial
with no spend. Use mock execution first. Real provider rehearsal requires separate
spend authorization.
