---
name: agent-browser
description: Compatibility discovery alias for A3S Use Browser automation. Use for navigating pages, filling forms, extracting data, testing web apps, Electron automation, Slack, Vercel Sandbox, or AWS AgentCore workflows.
allowed-tools: Bash(a3s:*)
hidden: true
---

# A3S Use Browser compatibility alias

Fast browser automation CLI for AI agents. Chrome/Chromium via CDP with accessibility-tree snapshots and compact `@eN` element refs.

Install: `a3s install use use/browser`

## Start here

This file preserves discovery for agents configured with the historical Skill name. Run the A3S command and load the version-matched workflow content:

```bash
a3s use browser skills get core             # start here — workflows, common patterns, troubleshooting
a3s use browser skills get core --full      # include full command reference and templates
```

The CLI serves skill content that always matches the installed version, so instructions never go stale. The content in this stub cannot change between releases, which is why it just points at `skills get core`.

## Specialized skills

Load a specialized skill when the task falls outside browser web pages:

```bash
a3s use browser skills get electron          # Electron desktop apps (VS Code, Slack, Discord, Figma, ...)
a3s use browser skills get slack             # Slack workspace automation
a3s use browser skills get dogfood           # Exploratory testing / QA / bug hunts
a3s use browser skills get vercel-sandbox    # Browser automation inside Vercel Sandbox microVMs
a3s use browser skills get agentcore         # AWS Bedrock AgentCore cloud browsers
```

Run `a3s use browser skills list` to see everything available on the installed version.

## Why A3S Use Browser

- Fast native Rust CLI, not a Node.js wrapper
- Works with any AI agent (Cursor, Claude Code, Codex, Continue, Windsurf, etc.)
- Chrome/Chromium via CDP with no Playwright or Puppeteer dependency
- Accessibility-tree snapshots with element refs for reliable interaction
- Sessions, authentication vault, state persistence, video recording
- Specialized skills for Electron apps, Slack, exploratory testing, cloud providers

## Observability Dashboard

The dashboard runs independently of browser sessions on port 4848. Agents should stay on the dashboard origin: session tabs, status, and stream traffic are proxied internally, so session ports do not need to be exposed.
