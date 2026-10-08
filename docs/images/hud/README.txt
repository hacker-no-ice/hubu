# HUD mock demonstration captures

These images show actual output from the compiled `hubu --color always hud`
command, captured through a pseudo-terminal and decoded with a VT100 terminal
emulator (`pyte`). The terminal cells, including ANSI change highlights, were
rendered into PNGs with a monospace font. Headings, the command caption, and the
mock/method labels are annotations outside the captured HUD output.

The CLI used an isolated temporary `HUBU_HOME`, a loopback HTTP mock endpoint,
and a fixture bearer token. Four read-only `GET /hud?currency=usd` responses
provided the demonstration states. No provider was called, no funds were spent,
and no existing stack profile was changed. These are illustrations of the
terminal presentation, not live-provider qualification or native Terminal app
screenshots. HUD behavior was unchanged from reviewed commit
`129561438c194dc604ef14340e2b0068eba49898`.

- [Reservation and exact settlement](reservation-and-settlement.png): an 8¢
  reservation, followed by 5.8¢ vendor cost and a conservative 6¢ budget charge.
- [Approval and policy denial](approval-and-policy-denial.png): an outstanding
  32¢ approval under `final_images`, followed by a 60¢ request blocked under
  `deny_over_50c`.

`size —` reflects the current spend-contract limitation: Hubu does not persist
image dimensions. The displayed mock provider names and policy rule identifiers
are fixture data. The terminal presentation comes from the actual
[HUD renderer](../../../crates/hubu-cli/src/hud.rs).
