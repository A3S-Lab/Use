---
name: a3s-use-browser
description: Browser automation CLI for AI agents. Use for website interaction, form filling, screenshots, extraction, QA, Electron apps, Slack, Vercel Sandbox, and AWS AgentCore browser workflows.
allowed-tools: Bash(a3s:*)
---

# A3S Use Browser

Use the host surface that is already available:

- In an A3S Code `use` worker, call the available
  `mcp__use_browser__*` tools directly. The host owns installation and MCP
  lifecycle; do not run component installation or shell commands there. Call
  `mcp__use_browser__agent_browser_doctor` first. If its managed browser is
  missing, request `mcp__use_browser__agent_browser_install`; the parent TUI
  must obtain HITL approval before that mutation can run.
- In a CLI-only agent host, use the `a3s use browser ...` commands below.

Install the built-in capability and its managed runtime when needed:

```bash
a3s install use use/browser
```

Load the version-matched core workflow before browser automation:

```bash
a3s use browser skills get core
a3s use browser skills get core --full
```

Specialized workflows are available through `skills get electron`, `slack`,
`dogfood`, `vercel-sandbox`, and `agentcore`. Run
`a3s use browser skills list` to inspect the installed inventory.
