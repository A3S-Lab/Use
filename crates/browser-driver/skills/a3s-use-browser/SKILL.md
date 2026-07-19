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

The first direct local launch installs the shared A3S-managed Chrome runtime
when no system or managed browser is available and first-use policy permits it.
Prepare it explicitly for deterministic startup or offline work:

```bash
a3s install use use/browser
```

Doctor, help, version, Skills, profiles, and MCP server startup remain
non-installing. `A3S_OFFLINE=1` and `A3S_NO_AUTO_INSTALL=1` prohibit the
first-use download.

Load the version-matched core workflow before browser automation:

```bash
a3s use browser skills get core
a3s use browser skills get core --full
```

Specialized workflows are available through `skills get electron`, `slack`,
`dogfood`, `vercel-sandbox`, and `agentcore`. Run
`a3s use browser skills list` to inspect the installed inventory.
