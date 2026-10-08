HUB-244 watch capture provenance

single-agent.png, multi-agent.png and compact.png show real output from the
compiled hubu watch command under a PTY against an isolated local HTTP mock.
The command is visible above each capture. A VT100 emulator decoded actual
ANSI output; emulator cells, including bold, were rendered verbatim with a
monospace font. The display is monochrome by design.
Native Terminal capture was unavailable, so these are faithful terminal-output
renders, not native window screenshots. Fixture timestamps are displayed in UTC.
Mock demonstration labels are outside CLI output. No provider calls, spend,
profile modifications, human capability tokens or production backend were used.
The fixtures cover frozen funds, sub-cent settlement, approval, policy denial,
and budget denial. Budget segment lengths follow the real renderer arithmetic.
