# Video Recording

Capture browser automation as video for debugging, documentation, or verification.

**Related**: [commands.md](commands.md) for full command reference, [SKILL.md](../SKILL.md) for quick start.

## Contents

- [Basic Recording](#basic-recording)
- [Recording Commands](#recording-commands)
- [Use Cases](#use-cases)
- [Best Practices](#best-practices)
- [Output Format](#output-format)
- [Limitations](#limitations)

## Basic Recording

```bash
# Launch the browser, then start recording
a3s use browser open https://example.com
a3s use browser record start ./demo.webm

# Perform actions
a3s use browser snapshot -i
a3s use browser click @e1
a3s use browser fill @e2 "test input"

# Stop and save
a3s use browser record stop
```

## Recording Commands

```bash
# Launch a session first
a3s use browser open

# Start recording to file
a3s use browser record start ./output.webm

# Stop current recording
a3s use browser record stop

# Restart with new file (stops current + starts new)
a3s use browser record restart ./take2.webm
```

## Use Cases

### Debugging Failed Automation

```bash
#!/bin/bash
# Record automation for debugging

# Run your automation
a3s use browser open https://app.example.com
a3s use browser record start ./debug-$(date +%Y%m%d-%H%M%S).webm
a3s use browser snapshot -i
a3s use browser click @e1 || {
    echo "Click failed - check recording"
    a3s use browser record stop
    exit 1
}

a3s use browser record stop
```

### Documentation Generation

```bash
#!/bin/bash
# Record workflow for documentation

a3s use browser open https://app.example.com/login
a3s use browser record start ./docs/how-to-login.webm
a3s use browser wait 1000  # Pause for visibility

a3s use browser snapshot -i
a3s use browser fill @e1 "demo@example.com"
a3s use browser wait 500

a3s use browser fill @e2 "password"
a3s use browser wait 500

a3s use browser click @e3
a3s use browser wait --load networkidle
a3s use browser wait 1000  # Show result

a3s use browser record stop
```

### CI/CD Test Evidence

```bash
#!/bin/bash
# Record E2E test runs for CI artifacts

TEST_NAME="${1:-e2e-test}"
RECORDING_DIR="./test-recordings"
mkdir -p "$RECORDING_DIR"

a3s use browser open
a3s use browser record start "$RECORDING_DIR/$TEST_NAME-$(date +%s).webm"

# Run test
if run_e2e_test; then
    echo "Test passed"
else
    echo "Test failed - recording saved"
fi

a3s use browser record stop
```

## Best Practices

### 1. Add Pauses for Clarity

```bash
# Slow down for human viewing
a3s use browser click @e1
a3s use browser wait 500  # Let viewer see result
```

### 2. Use Descriptive Filenames

```bash
# Include context in filename
a3s use browser record start ./recordings/login-flow-2024-01-15.webm
a3s use browser record start ./recordings/checkout-test-run-42.webm
```

### 3. Handle Recording in Error Cases

```bash
#!/bin/bash
set -e

cleanup() {
    a3s use browser record stop 2>/dev/null || true
    a3s use browser close 2>/dev/null || true
}
trap cleanup EXIT

a3s use browser open
a3s use browser record start ./automation.webm
# ... automation steps ...
```

### 4. Combine with Screenshots

```bash
# Record video AND capture key frames
a3s use browser open https://example.com
a3s use browser record start ./flow.webm
a3s use browser screenshot ./screenshots/step1-homepage.png

a3s use browser click @e1
a3s use browser screenshot ./screenshots/step2-after-click.png

a3s use browser record stop
```

## Output Format

- Default format: WebM (VP8/VP9 codec)
- Compatible with all modern browsers and video players
- Compressed but high quality

## Limitations

- Recording adds slight overhead to automation
- Large recordings can consume significant disk space
- Some headless environments may have codec limitations
