---
name: plain-skill
description: Checks a changelog against the git log. Use when the user asks whether CHANGELOG.md is up to date.
license: Apache-2.0
allowed-tools: Bash(git:*) Read
metadata:
  author: example-org
  version: "1.0"
---

# Changelog check

1. Run `scripts/changes.sh` to list the commits since the last tag.
2. Read `references/STYLE.md` for the entry format.
3. Say which commits have no entry.
