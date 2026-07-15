---
name: a3s-use-browser
description: Browser automation CLI for AI agents. Use for website interaction, form filling, screenshots, extraction, QA, Electron apps, Slack, Vercel Sandbox, and AWS AgentCore browser workflows.
allowed-tools: Bash(a3s:*)
---

# A3S Use Browser

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
