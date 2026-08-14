# Security Policy

Hubu is experimental, local-first software. This policy describes how to
report security problems; it is not a claim that Hubu is ready for production
or real-money use.

## Supported versions

Security fixes are made on a best-effort basis for the current `main` branch
and the latest GitHub release. Older releases and arbitrary commits are not
supported. Reporters should identify the exact commit or release they tested.

| Version | Supported |
| --- | --- |
| Current `main` | Yes |
| Latest GitHub release | Yes |
| Older releases | No |

## Report a vulnerability privately

Use the repository's **Security** tab and select **Report a vulnerability**.
That form creates a private vulnerability report visible only to the reporter
and repository maintainers. Include:

- the affected release or commit;
- the impact and the conditions needed to reproduce it;
- minimal reproduction steps or a proof of concept;
- any suggested mitigation; and
- whether you want public credit.

Do not open a public issue, pull request, discussion, or social-media post for
an uncoordinated vulnerability report. Do not include secrets, personal data,
or data belonging to another person.

GitHub private vulnerability reporting is a repository setting, not something
this file can enable. If **Report a vulnerability** is not visible, private
intake is not currently available through this repository. Please do not post
the details publicly; contact a maintainer through a private channel you
already trust and ask them to enable private vulnerability reporting. The
maintainers must not advertise this policy as an active private-reporting
channel until the GitHub setting is enabled.

## What to expect

Maintainers aim to:

- acknowledge a complete report within 3 business days;
- provide an initial severity and scope assessment within 10 business days;
- send a status update at least every 10 business days while a confirmed issue
  remains unresolved; and
- coordinate disclosure after a fix or practical mitigation is available.

These are response goals, not service-level guarantees. Hubu is maintained on
a best-effort basis, so remediation time depends on severity, complexity, and
maintainer availability. If a target is missed, reporters may request an
updated timeline through the private report.

## Safe disclosure

Please make a good-faith effort to avoid privacy violations, data destruction,
service disruption, social engineering, and access beyond what is necessary to
demonstrate the issue. Use test data and local environments where possible.
Give maintainers a reasonable opportunity to investigate and remediate before
public disclosure, and coordinate the timing and content of any advisory.

Maintainers will not request that a reporter conceal a vulnerability
indefinitely. If coordination stalls, state a proposed disclosure date in the
private report so both sides can work toward a safe, accurate disclosure.
