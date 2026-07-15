#!/bin/bash
# Template: Form Automation Workflow
# Purpose: Fill and submit web forms with validation
# Usage: ./form-automation.sh <form-url>
#
# This template demonstrates the snapshot-interact-verify pattern:
# 1. Navigate to form
# 2. Snapshot to get element refs
# 3. Fill fields using refs
# 4. Submit and verify result
#
# Customize: Update the refs (@e1, @e2, etc.) based on your form's snapshot output

set -euo pipefail

FORM_URL="${1:?Usage: $0 <form-url>}"

echo "Form automation: $FORM_URL"

# Step 1: Navigate to form
a3s use browser open "$FORM_URL"
a3s use browser wait --load networkidle

# Step 2: Snapshot to discover form elements
echo ""
echo "Form structure:"
a3s use browser snapshot -i

# Step 3: Fill form fields (customize these refs based on snapshot output)
#
# Common field types:
#   a3s use browser fill @e1 "John Doe"           # Text input
#   a3s use browser fill @e2 "user@example.com"   # Email input
#   a3s use browser fill @e3 "SecureP@ss123"      # Password input
#   a3s use browser select @e4 "Option Value"     # Dropdown
#   a3s use browser check @e5                     # Checkbox
#   a3s use browser click @e6                     # Radio button
#   a3s use browser fill @e7 "Multi-line text"   # Textarea
#   a3s use browser upload @e8 /path/to/file.pdf # File upload
#
# Uncomment and modify:
# a3s use browser fill @e1 "Test User"
# a3s use browser fill @e2 "test@example.com"
# a3s use browser click @e3  # Submit button

# Step 4: Wait for submission
# a3s use browser wait --load networkidle
# a3s use browser wait --url "**/success"  # Or wait for redirect

# Step 5: Verify result
echo ""
echo "Result:"
a3s use browser get url
a3s use browser snapshot -i

# Optional: Capture evidence
a3s use browser screenshot /tmp/form-result.png
echo "Screenshot saved: /tmp/form-result.png"

# Cleanup
a3s use browser close
echo "Done"
