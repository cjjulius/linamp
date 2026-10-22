---
description: End a LinAmp work session — commit check, then push linux-testing
allowed-tools: Bash(git branch:*), Bash(git status:*), Bash(git add:*), Bash(git commit:*), Bash(git push:*), Bash(git log:*), Bash(git rev-parse:*)
---
Wrap up a LinAmp work session on this machine so the other machine can pick up cleanly. Follow these steps:

1. Confirm the current branch is `linux-testing` (`git branch --show-current`). If it isn't, STOP and tell me where I am — don't push another branch.
2. Show `git status -sb`. If there are uncommitted changes, list them and ASK me whether to commit and for a commit message — do NOT commit automatically. When I confirm, author the commit as cjjulius with a plain subject + body and NO AI/assistant or `Co-Authored-By` attribution lines (match the repo's existing history).
3. Once the working tree is clean, push: `git push`.
4. Report the pushed commit range and confirm local `linux-testing` and `origin/linux-testing` now match.

If a push is rejected as non-fast-forward, do NOT force. Tell me, and recommend running `/begin-session` (or `git pull`) to integrate first.
