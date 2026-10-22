---
description: Start a LinAmp work session — sync linux-testing from the remote
allowed-tools: Bash(git switch:*), Bash(git branch:*), Bash(git fetch:*), Bash(git pull:*), Bash(git status:*), Bash(git log:*), Bash(git rev-parse:*)
---
Start a LinAmp work session on this machine. Follow these steps and report the result concisely (don't ask me anything unless step 2 fails):

1. Switch to the work branch: `git switch linux-testing`. If uncommitted changes block the switch, STOP and show me `git status -sb` — never discard or stash my changes without asking.
2. Fast-forward from the remote: `git pull --ff-only`.
3. If the fast-forward fails because the histories diverged, do NOT force, rebase, or merge on your own. Show me `git status -sb`, the diverging commits in each direction (`git log --oneline HEAD..@{u}` and `git log --oneline @{u}..HEAD`), explain what happened, and ask how I want to integrate.
4. On success, report the current HEAD (`git log --oneline -1`) and confirm the tree is clean and in sync with origin/linux-testing.
