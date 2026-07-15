# Session Management

Multiple isolated browser sessions with state persistence and concurrent browsing.

**Related**: [authentication.md](authentication.md) for login patterns, [SKILL.md](../SKILL.md) for quick start.

## Contents

- [Named Sessions](#named-sessions)
- [Session Isolation Properties](#session-isolation-properties)
- [Session State Persistence](#session-state-persistence)
- [Common Patterns](#common-patterns)
- [Default Session](#default-session)
- [Session Cleanup](#session-cleanup)
- [Best Practices](#best-practices)

## Named Sessions

Use `--session` to isolate browser contexts. Agent skills should derive one stable id and reuse it on every command:

```bash
SESSION="$(a3s use browser session id --scope worktree --prefix my-skill)"
a3s use browser --session "$SESSION" --restore open https://app.example.com/login
```

`--scope worktree` uses the Git worktree root when available, then the Git root, then the canonical current directory. This is the recommended default for agents because worktrees are commonly used for parallel agent runs.

```bash
# Session 1: Authentication flow
a3s use browser --session auth open https://app.example.com/login

# Session 2: Public browsing (separate cookies, storage)
a3s use browser --session public open https://example.com

# Commands are isolated by session
a3s use browser --session auth fill @e1 "user@example.com"
a3s use browser --session public get text body
```

## Session Isolation Properties

Each session has independent:
- Cookies
- LocalStorage / SessionStorage
- IndexedDB
- Cache
- Browsing history
- Open tabs

## Session State Persistence

### Automatic Restore

```bash
# Bare --restore uses the current --session as the persistence key
SESSION="$(a3s use browser session id --scope worktree --prefix next-dev-loop)"
a3s use browser --session "$SESSION" --restore open https://app.example.com/dashboard
```

State is loaded before navigation and saved on close, daemon shutdown, idle timeout, and compatible relaunch. It is also saved periodically while the browser is open (after commands settle, at most once per `A3S_USE_BROWSER_AUTOSAVE_INTERVAL_MS`, default 30000; set to `0` to save only on close), so a browser window the user closes by hand still leaves a recent save behind. Idle sessions keep saving on the same interval, capturing changes the page makes on its own such as token refreshes. The default save policy is `--restore-save auto`, which skips auto-save if restore failed or validation failed; `never` disables periodic autosave too.

```bash
a3s use browser --session "$SESSION" --restore --restore-check-url "**/dashboard" open https://app.example.com/dashboard
a3s use browser --session "$SESSION" --restore --restore-check-text Dashboard open https://app.example.com/dashboard
a3s use browser --session "$SESSION" --restore --restore-check-fn "!!localStorage.getItem('session')" open https://app.example.com/dashboard
```

Use `a3s use browser session info --json` for diagnostics:

```bash
a3s use browser --session "$SESSION" session info --json
```

### Manual State Files

Use `state save`, `state load`, and `--state <path>` when you need an explicit portable JSON file. Do not make agents construct paths under `~/.a3s use browser/sessions/`; prefer `--restore` for reusable agent sessions.

## Common Patterns

### Authenticated Session Reuse

```bash
#!/bin/bash
SESSION="$(a3s use browser session id --scope worktree --prefix app)"
a3s use browser --session "$SESSION" --restore open https://app.example.com/dashboard
```

### Concurrent Scraping

```bash
#!/bin/bash
# Scrape multiple sites concurrently

# Start all sessions
a3s use browser --session site1 open https://site1.com &
a3s use browser --session site2 open https://site2.com &
a3s use browser --session site3 open https://site3.com &
wait

# Extract from each
a3s use browser --session site1 get text body > site1.txt
a3s use browser --session site2 get text body > site2.txt
a3s use browser --session site3 get text body > site3.txt

# Cleanup
a3s use browser --session site1 close
a3s use browser --session site2 close
a3s use browser --session site3 close
```

### A/B Testing Sessions

```bash
# Test different user experiences
a3s use browser --session variant-a open "https://app.com?variant=a"
a3s use browser --session variant-b open "https://app.com?variant=b"

# Compare
a3s use browser --session variant-a screenshot /tmp/variant-a.png
a3s use browser --session variant-b screenshot /tmp/variant-b.png
```

## Default Session

When `--session` is omitted, commands use the default session:

```bash
# These use the same default session
a3s use browser open https://example.com
a3s use browser snapshot -i
a3s use browser close  # Closes default session
```

## Session Cleanup

```bash
# Close specific session
a3s use browser --session auth close

# List active sessions
a3s use browser session list
```

## Best Practices

### 1. Name Sessions Semantically

```bash
# GOOD: Clear purpose
a3s use browser --session github-auth open https://github.com
a3s use browser --session docs-scrape open https://docs.example.com

# AVOID: Generic names
a3s use browser --session s1 open https://github.com
```

### 2. Always Clean Up

```bash
# Close sessions when done
a3s use browser --session auth close
a3s use browser --session scrape close
```

### 3. Handle State Files Securely

```bash
# Don't commit state files (contain auth tokens!)
echo "*.auth-state.json" >> .gitignore

# Delete after use
rm /tmp/auth-state.json
```

### 4. Timeout Long Sessions

```bash
# Set timeout for automated scripts
timeout 60 a3s use browser --session long-task get text body
```
