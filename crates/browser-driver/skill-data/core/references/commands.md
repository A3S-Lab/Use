# Command Reference

Complete reference for all a3s use browser commands. For quick start and common patterns, see SKILL.md.

## Navigation

```bash
a3s use browser open            # Launch browser (no navigation); stays on about:blank.
                              # Pair with `network route`, `cookies set --curl`, or
                              # `addinitscript` to stage state before the first navigation.
a3s use browser open <url>      # Launch + navigate (aliases: goto, navigate)
                              # Supports: https://, http://, file://, about:, data://
                              # Auto-prepends https:// if no protocol given
a3s use browser read [url]      # Fetch agent-readable text, or read rendered active-tab DOM
                              # Explicit URLs send Accept: text/markdown, then try .md if needed
                              # Walks ancestor paths for llms.txt before HTML fallback
                              # --llms and --require-md without URL use the active tab URL
                              # --filter narrows page content to matching heading sections
                              # Honors --allowed-domains, --content-boundaries, and --max-output
                              # Options: --raw, --require-md, --outline, --llms <index|full>, --filter, --timeout <ms>
a3s use browser back            # Go back
a3s use browser forward         # Go forward
a3s use browser reload          # Reload page
a3s use browser pushstate <url> # SPA client-side navigation. Auto-detects
                              # window.next.router.push (triggers RSC fetch on Next.js);
                              # falls back to history.pushState + popstate/navigate events.
a3s use browser close           # Close browser (aliases: quit, exit)
a3s use browser connect 9222    # Connect to browser via CDP port
```

### Pre-navigation setup (one-turn batch)

```bash
a3s use browser batch \
  '["open"]' \
  '["network","route","*","--abort","--resource-type","script"]' \
  '["cookies","set","--curl","cookies.curl","--domain","localhost"]' \
  '["navigate","http://localhost:3000/target"]'
```

`open` with no URL gives you a clean launch so any interception, cookies, or init scripts you register take effect on the *first* real navigation. Use for SSR-only debug (`--resource-type script`), protected-origin auth, or capturing fresh `react suspense`/`vitals` state without noise from a prior page.

## Snapshot (page analysis)

```bash
a3s use browser snapshot            # Full accessibility tree
a3s use browser snapshot -i         # Interactive elements only (recommended)
a3s use browser snapshot -c         # Compact output
a3s use browser snapshot -d 3       # Limit depth to 3
a3s use browser snapshot -s "#main" # Scope to CSS selector
```

## Interactions (use @refs from snapshot)

```bash
a3s use browser click @e1           # Click
a3s use browser click @e1 --new-tab # Click and open in new tab
a3s use browser dblclick @e1        # Double-click
a3s use browser focus @e1           # Focus element
a3s use browser fill @e2 "text"     # Clear and type
a3s use browser type @e2 "text"     # Type without clearing
a3s use browser press Enter         # Press key (alias: key)
a3s use browser press Control+a     # Key combination
a3s use browser keydown Shift       # Hold key down
a3s use browser keyup Shift         # Release key
a3s use browser hover @e1           # Hover
a3s use browser check @e1           # Check checkbox
a3s use browser uncheck @e1         # Uncheck checkbox
a3s use browser select @e1 "value"  # Select dropdown option
a3s use browser select @e1 "a" "b"  # Select multiple options
a3s use browser scroll down 500     # Scroll page (default: down 300px)
a3s use browser scrollintoview @e1  # Scroll element into view (alias: scrollinto)
a3s use browser drag @e1 @e2        # Drag and drop
a3s use browser upload @e1 file.pdf # Upload files
```

Clicks fail before dispatch when another element covers the target's click point. The error names the covering element, for example `covered by <div#consent-banner>`. Dismiss or interact with that element, run a fresh snapshot, then retry the original action.

## Get Information

```bash
a3s use browser get text @e1        # Get element text
a3s use browser get html @e1        # Get innerHTML
a3s use browser get value @e1       # Get input value
a3s use browser get attr @e1 href   # Get attribute
a3s use browser get title           # Get page title
a3s use browser get url             # Get current URL
a3s use browser get cdp-url         # Get CDP WebSocket URL
a3s use browser get count ".item"   # Count matching elements
a3s use browser get box @e1         # Get bounding box
a3s use browser get styles @e1      # Get computed styles (font, color, bg, etc.)
```

## Check State

```bash
a3s use browser is visible @e1      # Check if visible
a3s use browser is enabled @e1      # Check if enabled
a3s use browser is checked @e1      # Check if checked
```

## Screenshots and PDF

```bash
a3s use browser screenshot          # Save to temporary directory
a3s use browser screenshot path.png # Save to specific path
a3s use browser screenshot --full   # Full page
a3s use browser pdf output.pdf      # Save as PDF
```

Headless Chromium screenshots hide native scrollbars for consistent image output. Pass `--hide-scrollbars false` when launching to keep native scrollbars visible.

## Video Recording

```bash
a3s use browser open https://example.com     # Launch a browser session first
a3s use browser record start ./demo.webm    # Start recording
a3s use browser click @e1                   # Perform actions
a3s use browser record stop                 # Stop and save video
a3s use browser record restart ./take2.webm # Stop current + start new
```

## Wait

```bash
a3s use browser wait @e1                     # Wait for element
a3s use browser wait 2000                    # Wait milliseconds
a3s use browser wait --text "Success"        # Wait for text (or -t)
a3s use browser wait --url "**/dashboard"    # Wait for URL pattern (or -u)
a3s use browser wait --load networkidle      # Wait for network idle (or -l)
a3s use browser wait --fn "window.ready"     # Wait for JS condition (or -f)
```

## Mouse Control

```bash
a3s use browser mouse move 100 200      # Move mouse
a3s use browser mouse down left         # Press button
a3s use browser mouse up left           # Release button
a3s use browser mouse wheel 100         # Scroll wheel
```

## Semantic Locators (alternative to refs)

```bash
a3s use browser find role button click --name "Submit"
a3s use browser find text "Sign In" click
a3s use browser find text "Sign In" click --exact      # Exact match only
a3s use browser find label "Email" fill "user@test.com"
a3s use browser find placeholder "Search" type "query"
a3s use browser find alt "Logo" click
a3s use browser find title "Close" click
a3s use browser find testid "submit-btn" click
a3s use browser find first ".item" click
a3s use browser find last ".item" click
a3s use browser find nth 2 "a" hover
```

## Browser Settings

```bash
a3s use browser set viewport 1920 1080          # Set viewport size
a3s use browser set viewport 1920 1080 2        # 2x retina (same CSS size, higher res screenshots)
a3s use browser set device "iPhone 14"          # Emulate device
a3s use browser set geo 37.7749 -122.4194       # Set geolocation (alias: geolocation)
a3s use browser set offline on                  # Toggle offline mode
a3s use browser set headers '{"X-Key":"v"}'     # Extra HTTP headers
a3s use browser set credentials user pass       # HTTP basic auth (alias: auth)
a3s use browser set media dark                  # Emulate color scheme
a3s use browser set media light reduced-motion  # Light mode + reduced motion
```

## Cookies and Storage

```bash
a3s use browser cookies                     # Get all cookies
a3s use browser cookies set name value      # Set cookie
a3s use browser cookies clear               # Clear cookies
a3s use browser storage local               # Get all localStorage
a3s use browser storage local key           # Get specific key
a3s use browser storage local set k v       # Set value
a3s use browser storage local clear         # Clear all
```

## Network

```bash
a3s use browser network route <url>              # Intercept requests
a3s use browser network route <url> --abort      # Block requests
a3s use browser network route <url> --body '{}'  # Mock response
a3s use browser network unroute [url]            # Remove routes
a3s use browser network requests                 # View tracked requests
a3s use browser network requests --filter api    # Filter requests
```

## Tabs and Windows

```bash
a3s use browser tab                              # List tabs with tabId and label
a3s use browser tab new [url]                    # New tab
a3s use browser tab new --label docs [url]       # New tab with a memorable label
a3s use browser tab t2                           # Switch to tab by id
a3s use browser tab docs                         # Switch to tab by label
a3s use browser tab close                        # Close current tab
a3s use browser tab close t2                     # Close tab by id
a3s use browser tab close docs                   # Close tab by label
a3s use browser window new                       # New window
```

Tab ids are stable strings of the form `t1`, `t2`, `t3`. They're never reused within a session, so the same id keeps referring to the same tab across commands. Positional integers are **not** accepted — `tab 2` errors with a teaching message; use `t2`.

User-assigned labels (`docs`, `app`, `admin`) are interchangeable with ids everywhere a tab ref is accepted. Labels are the agent-friendly way to write multi-tab workflows:

```bash
a3s use browser tab new --label docs https://docs.example.com
a3s use browser tab new --label app  https://app.example.com
a3s use browser tab docs                   # switch to docs
a3s use browser snapshot                   # populate refs for docs
a3s use browser click @e1                  # ref click on docs
a3s use browser tab app                    # switch to app
a3s use browser tab close docs             # close by label
```

Labels are never auto-generated, never rewritten on navigation, and must be unique within a session. To interact with another tab, switch to it first: the daemon maintains a single active tab, so refs (`@eN`) belong to the tab that was active when the snapshot ran.

## Frames

```bash
a3s use browser frame "#iframe"     # Switch to iframe by CSS selector
a3s use browser frame @e3           # Switch to iframe by element ref
a3s use browser frame main          # Back to main frame
```

### Iframe support

Iframes are detected automatically during snapshots. When the main-frame snapshot runs, `Iframe` nodes are resolved and their content is inlined beneath the iframe element in the output (one level of nesting; iframes within iframes are not expanded).

```bash
a3s use browser snapshot -i
# @e3 [Iframe] "payment-frame"
#   @e4 [input] "Card number"
#   @e5 [button] "Pay"

# Interact directly — refs inside iframes already work
a3s use browser fill @e4 "4111111111111111"
a3s use browser click @e5

# Or switch frame context for scoped snapshots
a3s use browser frame @e3               # Switch using element ref
a3s use browser snapshot -i             # Snapshot scoped to that iframe
a3s use browser frame main              # Return to main frame
```

The `frame` command accepts:
- **Element refs** — `frame @e3` resolves the ref to an iframe element
- **CSS selectors** — `frame "#payment-iframe"` finds the iframe by selector
- **Frame name/URL** — matches against the browser's frame tree

## Dialogs

By default, `alert` and `beforeunload` dialogs are automatically accepted so they never block the agent. `confirm` and `prompt` dialogs still require explicit handling. Use `--no-auto-dialog` to disable this behavior.

```bash
a3s use browser dialog accept [text]  # Accept dialog
a3s use browser dialog dismiss        # Dismiss dialog
a3s use browser dialog status         # Check if a dialog is currently open
```

## JavaScript

```bash
a3s use browser eval "document.title"          # Simple expressions only
a3s use browser eval -b "<base64>"             # Any JavaScript (base64 encoded)
a3s use browser eval --stdin                   # Read script from stdin
```

Use `-b`/`--base64` or `--stdin` for reliable execution. Shell escaping with nested quotes and special characters is error-prone.

```bash
# Base64 encode your script, then:
a3s use browser eval -b "ZG9jdW1lbnQucXVlcnlTZWxlY3RvcignW3NyYyo9Il9uZXh0Il0nKQ=="

# Or use stdin with heredoc for multiline scripts:
cat <<'EOF' | a3s use browser eval --stdin
const links = document.querySelectorAll('a');
Array.from(links).map(a => a.href);
EOF
```

## Authentication and Plugins

```bash
a3s use browser auth save <name> --url <url> --username <user> --password-stdin
a3s use browser auth login <name>          # Login using saved credentials
a3s use browser auth login <name> --credential-provider <plugin> [--item <ref>] [--url <url>]
a3s use browser auth login <name> --username-selector <s> --password-selector <s> [--submit-selector <s>]
a3s use browser auth list                  # List saved auth profiles
a3s use browser auth show <name>           # Show profile metadata, no passwords
a3s use browser auth delete <name>         # Delete a saved profile
a3s use browser plugin add <ref>           # Add a plugin from npm or GitHub
a3s use browser plugin list                # List configured plugins
a3s use browser plugin show <name>         # Show one configured plugin
a3s use browser plugin run <name> <type> --payload <json>
                                          # Run an arbitrary plugin request
```

Credential provider plugins run out-of-process over the `a3s use browser.plugin.v1` stdio JSON protocol and must declare `credential.read`. Use `--confirm-actions plugin:<name>:credential.read` to require explicit approval before a plugin resolves secrets.

Other capabilities use the same protocol:
- `browser.provider`: `a3s use browser --provider <name> open <url>`
- `launch.mutate`: append local launch args, extensions, or init scripts
- `command.run`: `a3s use browser plugin run <name> <type> --payload <json>`

`plugin run` is for `command.run` and custom capabilities. Core capabilities and protocol request types use their dedicated command paths.

## State Management

```bash
a3s use browser state save auth.json    # Save cookies, storage, auth state
a3s use browser state load auth.json    # Restore saved state
```

## MCP Server

```bash
a3s use browser mcp
a3s use browser mcp --tools all
a3s use browser mcp --tools core,network,react
```

Starts a stdio Model Context Protocol server. MCP clients should configure the server command as `a3s use browser` with args `["mcp"]`. The server defaults to MCP protocol 2025-11-25 and accepts older supported client protocol versions during initialization.

The default tools profile is `core`, which keeps MCP context small for everyday browser automation. Use `--tools all` for the full typed CLI parity surface, or combine profiles with commas, such as `--tools core,network,react`.

Profiles:

- `core` - Default. Navigation, snapshots, interaction, waits, reads, screenshots, JavaScript eval, close, tab basics, and profile discovery
- `network` - Network routes, request inspection, HAR, headers, credentials, offline
- `state` - Cookies, storage, auth, saved state, sessions, profiles, skills
- `debug` - Console/errors, tracing, profiling, recording, clipboard, plugins, doctor, dashboard, install, upgrade, chat, diff, batch, confirm/deny
- `tabs` - Back/forward/reload, tabs, windows, frames, dialogs
- `react` - React tree/inspect/renders/suspense, vitals, pushstate
- `mobile` - Viewport/device/geolocation/media, touch, swipe, mouse, keyboard
- `all` - Every MCP tool, including the full typed CLI parity surface

Common tools include:

- `agent_browser_tools_profiles`
- `agent_browser_open`
- `agent_browser_snapshot`
- `agent_browser_click`
- `agent_browser_fill`
- `agent_browser_type`
- `agent_browser_press`
- `agent_browser_wait_for_selector`
- `agent_browser_screenshot`
- `agent_browser_get_url`
- `agent_browser_eval`
- `agent_browser_close`

Tool calls use the same config files and environment variables as the CLI. Each tool accepts typed arguments plus `extraArgs` for advanced CLI flags and exact CLI parity. Tool discovery is paginated and includes read-only/open-world annotations so modern MCP clients can load the large typed surface incrementally. Use the `session` tool argument or `A3S_USE_BROWSER_SESSION` to isolate browser state.

## Global Options

```bash
a3s use browser --session <name> ...    # Isolated browser session
a3s use browser --json ...              # JSON output for parsing
a3s use browser --headed ...            # Show browser window (not headless; on displayless Linux an Xvfb display starts automatically)
a3s use browser --webgpu ...            # Enable WebGPU (SwiftShader software Vulkan on Linux, no GPU needed)
a3s use browser --cdp <port> ...        # Connect via Chrome DevTools Protocol
a3s use browser -p <provider> ...       # Browser provider or configured provider plugin
a3s use browser --proxy <url> ...       # Use proxy server
a3s use browser --proxy-bypass <hosts>  # Hosts to bypass proxy
a3s use browser --headers <json> ...    # HTTP headers scoped to URL's origin
a3s use browser --executable-path <p>   # Custom browser executable
a3s use browser --extension <path> ...  # Load browser extension (repeatable)
a3s use browser --ignore-https-errors   # Ignore SSL certificate errors
a3s use browser --hide-scrollbars false # Keep native scrollbars visible in headless Chromium screenshots
a3s use browser --help                  # Show help (-h)
a3s use browser --version               # Show version (-V)
a3s use browser <command> --help        # Show detailed help for a command
```

## Debugging

```bash
a3s use browser --headed open example.com   # Show browser window
a3s use browser --cdp 9222 snapshot         # Connect via CDP port
a3s use browser connect 9222                # Alternative: connect command
a3s use browser console                     # View console messages
a3s use browser console --clear             # Clear console
a3s use browser errors                      # View page errors
a3s use browser errors --clear              # Clear errors
a3s use browser highlight @e1               # Highlight element
a3s use browser inspect                     # Open Chrome DevTools for this session
a3s use browser trace start                 # Start recording trace
a3s use browser trace stop trace.json       # Stop and save trace
a3s use browser profiler start              # Start Chrome DevTools profiling
a3s use browser profiler stop trace.json    # Stop and save profile
```

## React / Web Vitals

Requires `--enable react-devtools` at launch for the `react ...` commands. `vitals` and `pushstate` are framework-agnostic.

```bash
a3s use browser open --enable react-devtools <url>    # Launch with React hook installed
a3s use browser react tree                            # Full component tree
a3s use browser react inspect <fiberId>               # Props, hooks, state, source
a3s use browser react renders start                   # Begin re-render recording
a3s use browser react renders stop [--json]           # Stop and print render profile
a3s use browser react suspense [--only-dynamic] [--json]  # Suspense boundaries + classifier
                                                         # --only-dynamic hides the "static" list
a3s use browser vitals [url] [--json]                 # LCP/CLS/TTFB/FCP/INP + hydration
a3s use browser pushstate <url>                       # SPA client-side nav (auto-detects Next router)
```

`vitals` prints a summary by default and uses the same fields as the structured `--json` response.

## Init scripts

```bash
a3s use browser open --init-script <path>             # Register before first navigation (repeatable)
a3s use browser addinitscript <js>                    # Register at runtime (returns identifier)
a3s use browser removeinitscript <identifier>         # Remove a previously registered init script
```

## cURL cookie import

```bash
a3s use browser cookies set --curl <file>                             # Auto-detects JSON/cURL/Cookie-header
a3s use browser cookies set --curl <file> --domain example.com        # Scope to a domain
```

Supported formats: JSON array of `{name, value}`, a cURL dump from DevTools -> Network -> Copy as cURL, or a bare Cookie header. Errors never echo cookie values.

## Network route by resource type

```bash
a3s use browser network route '*' --abort --resource-type script       # Block scripts only (SSR-lock pattern)
a3s use browser network route '*' --resource-type image,font --body '' # Stub images and fonts
```

## Environment Variables

```bash
A3S_USE_BROWSER_SESSION="mysession"            # Default session name
A3S_USE_BROWSER_EXECUTABLE_PATH="/path/chrome" # Custom browser path
A3S_USE_BROWSER_EXTENSIONS="/ext1,/ext2"       # Comma-separated extension paths
A3S_USE_BROWSER_INIT_SCRIPTS="/a.js,/b.js"     # Comma-separated init script paths
A3S_USE_BROWSER_ENABLE="react-devtools"        # Comma-separated built-in init script features
A3S_USE_BROWSER_HIDE_SCROLLBARS="false"        # Keep native scrollbars visible in headless Chromium screenshots
A3S_USE_BROWSER_WEBGPU="1"                     # Enable the WebGPU launch preset (see references/webgpu.md)
A3S_USE_BROWSER_NO_XVFB="1"                    # Disable automatic Xvfb for headed mode on displayless Linux
A3S_USE_BROWSER_PROVIDER="browserbase"         # Browser provider or configured provider plugin
A3S_USE_BROWSER_STREAM_PORT="9223"             # Override WebSocket streaming port (default: OS-assigned)
A3S_USE_BROWSER_CONFIG="./a3s use browser.json"  # Custom config file
A3S_USE_BROWSER_CDP="9222"                     # Connect daemon to CDP port or WebSocket URL
A3S_USE_BROWSER_PLUGINS='[{"name":"vault","command":"agent-browser-plugin-vault","capabilities":["credential.read"]},{"name":"stealth","command":"agent-browser-plugin-stealth","capabilities":["launch.mutate"]}]'
```
