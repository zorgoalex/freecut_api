#!/usr/bin/env python3
"""Check that Russian and English research logs stay structurally synchronized."""

from __future__ import annotations

import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
RU_LOG = ROOT / "docs" / "research" / "cutting-optimization-research-log.ru.md"
EN_LOG = ROOT / "docs" / "research" / "cutting-optimization-research-log.en.md"


def read_sync_index(path: Path) -> list[str]:
    text = path.read_text(encoding="utf-8")
    match = re.search(r"<!--\s*research-log-sync-index\s*(.*?)\s*-->", text, re.DOTALL)
    if not match:
        raise ValueError(f"{path} does not contain research-log-sync-index")
    return [line.strip() for line in match.group(1).splitlines() if line.strip()]


def main() -> int:
    ru_index = read_sync_index(RU_LOG)
    en_index = read_sync_index(EN_LOG)
    if ru_index != en_index:
        print("Research log sync check failed: RU and EN indexes differ.", file=sys.stderr)
        print(f"RU only: {sorted(set(ru_index) - set(en_index))}", file=sys.stderr)
        print(f"EN only: {sorted(set(en_index) - set(ru_index))}", file=sys.stderr)
        for idx, (ru_item, en_item) in enumerate(zip(ru_index, en_index), start=1):
            if ru_item != en_item:
                print(f"First mismatch at #{idx}: RU={ru_item!r}, EN={en_item!r}", file=sys.stderr)
                break
        return 1
    print(f"Research log sync OK: {len(ru_index)} sections.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
