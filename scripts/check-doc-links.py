#!/usr/bin/env python3
"""Validate local documentation and architecture source links."""

from __future__ import annotations

import re
from pathlib import Path
from urllib.parse import unquote


ROOT = Path(__file__).resolve().parent.parent
MARKDOWN_LINK = re.compile(r"!?\[[^\]]*\]\(([^)\s]+)(?:\s+[^)]*)?\)")
REFERENCE_LINK = re.compile(r"^\[[^\]]+\]:\s*(\S+)", re.MULTILINE)
ARCHITECTURE_PATH = re.compile(
    r'(?:\["[^"]+",|\bpath:)\s*"((?:\.github|architecture|crates|docs|examples|skills)/[^"]+)"'
)
HTML_ASSET = re.compile(r'(?:href|src)="((?:\./|\.\./)[^"]+)"')


def local_target(raw_target: str, source: Path) -> Path | None:
    target = raw_target.strip("<>")
    if target.startswith(("#", "/", "mailto:", "http://", "https://")):
        return None
    target = unquote(target.split("#", 1)[0].split("?", 1)[0])
    if not target:
        return None
    return (source.parent / target).resolve()


def main() -> int:
    checked = 0
    failures: list[str] = []

    for source in sorted(ROOT.rglob("*.md")):
        ignored_directories = {".git", "target", "node_modules", "dist", ".vinext"}
        if any(part in ignored_directories for part in source.parts):
            continue
        text = source.read_text(encoding="utf-8")
        for match in (*MARKDOWN_LINK.finditer(text), *REFERENCE_LINK.finditer(text)):
            target = local_target(match.group(1), source)
            if target is None:
                continue
            checked += 1
            if not target.exists():
                failures.append(
                    f"{source.relative_to(ROOT)} -> {match.group(1)}"
                )

    architecture_js = ROOT / "architecture/architecture.js"
    for match in ARCHITECTURE_PATH.finditer(
        architecture_js.read_text(encoding="utf-8")
    ):
        checked += 1
        if not (ROOT / match.group(1)).exists():
            failures.append(
                f"{architecture_js.relative_to(ROOT)} -> {match.group(1)}"
            )

    architecture_html = ROOT / "architecture/index.html"
    for match in HTML_ASSET.finditer(
        architecture_html.read_text(encoding="utf-8")
    ):
        target = local_target(match.group(1), architecture_html)
        if target is None:
            continue
        checked += 1
        if not target.exists():
            failures.append(
                f"{architecture_html.relative_to(ROOT)} -> {match.group(1)}"
            )

    if failures:
        print("Broken local documentation links:")
        for failure in failures:
            print(f"- {failure}")
        return 1

    print(f"validated {checked} local documentation and architecture links")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
