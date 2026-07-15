use std::sync::OnceLock;

use crate::color;
use crate::connection::Response;

static BOUNDARY_NONCE: OnceLock<String> = OnceLock::new();

/// Per-process nonce for content boundary markers. Uses a CSPRNG (getrandom) so
/// that untrusted page content cannot predict or spoof the boundary delimiter.
/// Process ID or timestamps would be insufficient since pages can read those.
fn get_boundary_nonce() -> &'static str {
    BOUNDARY_NONCE.get_or_init(|| {
        let mut buf = [0u8; 16];
        getrandom::getrandom(&mut buf).expect("failed to generate random nonce");
        buf.iter().map(|b| format!("{:02x}", b)).collect()
    })
}

#[derive(Default)]
pub struct OutputOptions {
    pub json: bool,
    pub content_boundaries: bool,
    pub max_output: Option<usize>,
}

impl OutputOptions {
    pub fn from_flags(flags: &crate::flags::Flags) -> Self {
        Self {
            json: flags.json,
            content_boundaries: flags.content_boundaries,
            max_output: flags.max_output,
        }
    }
}

fn truncate_if_needed(content: &str, max: Option<usize>) -> String {
    let Some(limit) = max else {
        return content.to_string();
    };
    // Fast path: byte length is a lower bound on char count, so if the
    // byte length is within the limit the char count must be too.
    if content.len() <= limit {
        return content.to_string();
    }
    // Find the byte offset of the limit-th character.
    match content.char_indices().nth(limit).map(|(i, _)| i) {
        Some(byte_offset) => {
            let total_chars = content.chars().count();
            format!(
                "{}\n[truncated: showing {} of {} chars. Use --max-output to adjust]",
                &content[..byte_offset],
                limit,
                total_chars
            )
        }
        // Content has fewer than `limit` chars despite more bytes
        None => content.to_string(),
    }
}

fn format_with_boundaries(content: &str, origin: Option<&str>, opts: &OutputOptions) -> String {
    let content = truncate_if_needed(content, opts.max_output);
    if opts.content_boundaries {
        let origin_str = origin.unwrap_or("unknown");
        let nonce = get_boundary_nonce();
        format!(
            "--- AGENT_BROWSER_PAGE_CONTENT nonce={} origin={} ---",
            nonce, origin_str
        ) + "\n"
            + &content
            + "\n"
            + &format!("--- END_AGENT_BROWSER_PAGE_CONTENT nonce={} ---", nonce)
    } else {
        content
    }
}

fn print_with_boundaries(content: &str, origin: Option<&str>, opts: &OutputOptions) {
    let content = format_with_boundaries(content, origin, opts);
    print!("{}", content);
    if !content.ends_with('\n') {
        println!();
    }
}

fn boundary_origin(data: &serde_json::Value) -> Option<&str> {
    for key in ["origin", "finalUrl", "url"] {
        if let Some(value) = data.get(key).and_then(|v| v.as_str()) {
            return Some(value);
        }
    }
    None
}

fn format_storage_value(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map(ToString::to_string)
        .unwrap_or_else(|| serde_json::to_string(value).unwrap_or_default())
}

fn format_storage_text(data: &serde_json::Value) -> Option<String> {
    if let Some(entries) = data.get("data").and_then(|v| v.as_object()) {
        if entries.is_empty() {
            return Some("No storage entries".to_string());
        }

        let lines = entries
            .iter()
            .map(|(key, value)| format!("{}: {}", key, format_storage_value(value)))
            .collect::<Vec<_>>();
        return Some(lines.join("\n"));
    }

    let key = data.get("key").and_then(|v| v.as_str())?;
    let value = data.get("value")?;
    Some(format!("{}: {}", key, format_storage_value(value)))
}

fn format_stream_status_text(action: Option<&str>, data: &serde_json::Value) -> Option<String> {
    match action {
        Some("stream_disable") => data
            .get("disabled")
            .and_then(|v| v.as_bool())
            .filter(|disabled| *disabled)
            .map(|_| "Streaming disabled".to_string()),
        Some("stream_enable") | Some("stream_status") => {
            let enabled = data.get("enabled").and_then(|v| v.as_bool())?;
            if !enabled {
                return Some("Streaming disabled".to_string());
            }

            let port = data.get("port").and_then(|v| v.as_u64())?;
            let connected = data
                .get("connected")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let screencasting = data
                .get("screencasting")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            Some(format!(
                "Streaming enabled on ws://127.0.0.1:{port}\nConnected: {connected}\nScreencasting: {screencasting}"
            ))
        }
        _ => None,
    }
}

fn confirmation_data(data: &serde_json::Value) -> Option<&serde_json::Value> {
    if data
        .get("confirmation_required")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Some(data);
    }

    data.get("result")
        .and_then(|v| v.get("data"))
        .and_then(confirmation_data)
}

fn print_confirmation_required(data: &serde_json::Value) {
    let action = data.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let category = data.get("category").and_then(|v| v.as_str()).unwrap_or("");
    let description = data
        .get("description")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(action);
    let cid = data
        .get("confirmation_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(action);

    println!("Confirmation required:");
    if category.is_empty() {
        println!("  {}", description);
    } else {
        println!("  {}: {}", category, description);
    }
    println!("  Run: a3s use browser confirm {}", cid);
    println!("  Or:  a3s use browser deny {}", cid);
}

fn format_metric_ms(value: Option<f64>) -> String {
    value
        .map(|v| format!("{}ms", format_compact_number(v)))
        .unwrap_or_else(|| "-".to_string())
}

fn format_compact_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        let formatted = format!("{:.2}", value);
        formatted
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

fn truncate_field(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let mut truncated: String = value.chars().take(max_chars.saturating_sub(3)).collect();
    truncated.push_str("...");
    truncated
}

fn format_vitals_text(data: &serde_json::Value) -> String {
    let url = data.get("url").and_then(|v| v.as_str()).unwrap_or("-");
    let ttfb = format_metric_ms(data.get("ttfb").and_then(|v| v.as_f64()));
    let fcp = format_metric_ms(data.get("fcp").and_then(|v| v.as_f64()));
    let inp = format_metric_ms(data.get("inp").and_then(|v| v.as_f64()));
    let lcp = data
        .get("lcp")
        .and_then(|v| v.get("startTime"))
        .and_then(|v| v.as_f64());
    let lcp = format_metric_ms(lcp);
    let cls = data
        .get("cls")
        .and_then(|v| v.get("score"))
        .and_then(|v| v.as_f64())
        .map(format_compact_number)
        .unwrap_or_else(|| "-".to_string());

    let mut lines = vec![
        format!("url: {}", url),
        format!(
            "ttfb: {}  fcp: {}  lcp: {}  cls: {}  inp: {}",
            ttfb, fcp, lcp, cls, inp
        ),
    ];

    if let Some(lcp_data) = data.get("lcp").and_then(|v| v.as_object()) {
        let element = lcp_data.get("element").and_then(|v| v.as_str());
        let lcp_url = lcp_data.get("url").and_then(|v| v.as_str());
        if element.is_some() || lcp_url.is_some() {
            let mut parts = Vec::new();
            if let Some(element) = element {
                parts.push(format!("element: {}", element));
            }
            if let Some(lcp_url) = lcp_url {
                parts.push(format!("asset: {}", truncate_field(lcp_url, 96)));
            }
            lines.push(format!("lcp: {}", parts.join("  ")));
        }
    }

    let phase_count = data
        .get("phases")
        .and_then(|v| v.as_array())
        .map(|v| v.len())
        .unwrap_or(0);
    let component_count = data
        .get("hydratedComponents")
        .and_then(|v| v.as_array())
        .map(|v| v.len())
        .unwrap_or(0);
    let hydration = data
        .get("hydration")
        .and_then(|v| v.get("duration"))
        .and_then(|v| v.as_f64());
    lines.push(format!(
        "hydration: {}  phases: {}  hydratedComponents: {}",
        format_metric_ms(hydration),
        phase_count,
        component_count
    ));

    lines.join("\n")
}

pub fn print_response_with_opts(resp: &Response, action: Option<&str>, opts: &OutputOptions) {
    if opts.json {
        if opts.content_boundaries {
            let mut json_val = serde_json::to_value(resp).unwrap_or_default();
            if let Some(obj) = json_val.as_object_mut() {
                let nonce = get_boundary_nonce();
                let origin = obj
                    .get("data")
                    .and_then(boundary_origin)
                    .unwrap_or("unknown");
                obj.insert(
                    "_boundary".to_string(),
                    serde_json::json!({
                        "nonce": nonce,
                        "origin": origin,
                    }),
                );
            }
            println!("{}", serde_json::to_string(&json_val).unwrap_or_default());
        } else {
            println!("{}", serde_json::to_string(resp).unwrap_or_default());
        }
        // JSON mode includes the warning field in the JSON payload already
        return;
    }

    if !resp.success {
        eprintln!(
            "{} {}",
            color::error_indicator(),
            resp.error.as_deref().unwrap_or("Unknown error")
        );
        // Still print dialog warning after errors, since a pending dialog
        // is the most common cause of commands timing out
        if let Some(ref warning) = resp.warning {
            eprintln!("{} {}", color::warning_indicator(), warning);
        }
        return;
    }

    if let Some(data) = &resp.data {
        print_lifecycle_note(data);

        // Dialog status response
        if action == Some("dialog") {
            if let Some(has_dialog) = data.get("hasDialog").and_then(|v| v.as_bool()) {
                if has_dialog {
                    let dtype = data
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let message = data.get("message").and_then(|v| v.as_str()).unwrap_or("");
                    println!(
                        "{} JavaScript {} dialog is open: \"{}\"",
                        color::warning_indicator(),
                        dtype,
                        message
                    );
                    if let Some(default_prompt) = data.get("defaultPrompt").and_then(|v| v.as_str())
                    {
                        println!("  Default prompt text: \"{}\"", default_prompt);
                    }
                    println!("  Use `dialog accept [text]` or `dialog dismiss` to resolve it");
                } else {
                    println!("{} No dialog is currently open", color::success_indicator());
                }
                print_warning(resp);
                return;
            }
        }
        if let Some(output) = format_stream_status_text(action, data) {
            println!("{}", output);
            return;
        }
        if action == Some("vitals") {
            println!("{}", format_vitals_text(data));
            return;
        }
        if action == Some("storage_get") {
            if let Some(output) = format_storage_text(data) {
                println!("{}", output);
                return;
            }
        }
        // Inspect response (check before generic URL handler since it also has a "url" field)
        if action == Some("inspect") {
            let opened = data
                .get("opened")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if opened {
                if let Some(url) = data.get("url").and_then(|v| v.as_str()) {
                    println!("{} Opened DevTools: {}", color::success_indicator(), url);
                } else {
                    println!("{} Opened DevTools", color::success_indicator());
                }
            } else if let Some(err) = data.get("error").and_then(|v| v.as_str()) {
                eprintln!("Could not open DevTools: {}", err);
            }
            return;
        }
        if action == Some("read") {
            if let Some(content) = data.get("content").and_then(|v| v.as_str()) {
                let origin = data
                    .get("finalUrl")
                    .and_then(|v| v.as_str())
                    .or_else(|| data.get("url").and_then(|v| v.as_str()));
                print_with_boundaries(content, origin, opts);
            }
            return;
        }
        // Navigation response
        if let Some(url) = data.get("url").and_then(|v| v.as_str()) {
            if let Some(title) = data.get("title").and_then(|v| v.as_str()) {
                println!("{} {}", color::success_indicator(), color::bold(title));
                println!("  {}", color::dim(url));
                return;
            }
            println!("{}", url);
            return;
        }
        if let Some(cdp_url) = data.get("cdpUrl").and_then(|v| v.as_str()) {
            println!("{}", cdp_url);
            return;
        }
        // Rich command reports (React renders/suspense and older daemon responses)
        if let Some(report) = data.get("report").and_then(|v| v.as_str()) {
            println!("{}", report);
            return;
        }
        // Diff responses -- route by action to avoid fragile shape probing
        if let Some(obj) = data.as_object() {
            match action {
                Some("diff_snapshot") => {
                    print_snapshot_diff(obj);
                    return;
                }
                Some("diff_screenshot") => {
                    print_screenshot_diff(obj);
                    return;
                }
                Some("diff_url") => {
                    if let Some(snap_data) = obj.get("snapshot").and_then(|v| v.as_object()) {
                        println!("{}", color::bold("Snapshot diff:"));
                        print_snapshot_diff(snap_data);
                    }
                    if let Some(ss_data) = obj.get("screenshot").and_then(|v| v.as_object()) {
                        println!("\n{}", color::bold("Screenshot diff:"));
                        print_screenshot_diff(ss_data);
                    }
                    return;
                }
                _ => {}
            }
        }
        let origin = data.get("origin").and_then(|v| v.as_str());
        // Snapshot
        if let Some(snapshot) = data.get("snapshot").and_then(|v| v.as_str()) {
            print_with_boundaries(snapshot, origin, opts);
            return;
        }
        // Title
        if let Some(title) = data.get("title").and_then(|v| v.as_str()) {
            println!("{}", title);
            return;
        }
        // Text
        if let Some(text) = data.get("text").and_then(|v| v.as_str()) {
            print_with_boundaries(text, origin, opts);
            return;
        }
        // HTML
        if let Some(html) = data.get("html").and_then(|v| v.as_str()) {
            print_with_boundaries(html, origin, opts);
            return;
        }
        // Value
        if let Some(value) = data.get("value").and_then(|v| v.as_str()) {
            println!("{}", value);
            return;
        }
        // Count
        if let Some(count) = data.get("count").and_then(|v| v.as_i64()) {
            println!("{}", count);
            return;
        }
        // Bounding box (get box)
        if action == Some("boundingbox") {
            if let Some(obj) = data.as_object() {
                let x = obj.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let y = obj.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let w = obj.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let h = obj.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
                println!("x:      {}", x);
                println!("y:      {}", y);
                println!("width:  {}", w);
                println!("height: {}", h);
            }
            return;
        }
        // Computed styles (get styles)
        if let Some(styles) = data.get("styles").and_then(|v| v.as_object()) {
            for (key, val) in styles {
                let display = match val.as_str() {
                    Some(s) => s.to_string(),
                    None => val.to_string(),
                };
                println!("{}: {}", key, display);
            }
            return;
        }
        // Boolean results
        if let Some(visible) = data.get("visible").and_then(|v| v.as_bool()) {
            println!("{}", visible);
            return;
        }
        if let Some(enabled) = data.get("enabled").and_then(|v| v.as_bool()) {
            println!("{}", enabled);
            return;
        }
        if let Some(checked) = data.get("checked").and_then(|v| v.as_bool()) {
            println!("{}", checked);
            return;
        }
        // Eval result
        if let Some(result) = data.get("result") {
            let formatted = serde_json::to_string_pretty(result).unwrap_or_default();
            print_with_boundaries(&formatted, origin, opts);
            return;
        }
        // iOS Devices
        if let Some(devices) = data.get("devices").and_then(|v| v.as_array()) {
            if devices.is_empty() {
                println!("No iOS devices available. Open Xcode to download simulator runtimes.");
                return;
            }

            // Separate real devices from simulators
            let real_devices: Vec<_> = devices
                .iter()
                .filter(|d| {
                    d.get("isRealDevice")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                })
                .collect();
            let simulators: Vec<_> = devices
                .iter()
                .filter(|d| {
                    !d.get("isRealDevice")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                })
                .collect();

            if !real_devices.is_empty() {
                println!("Connected Devices:\n");
                for device in real_devices.iter() {
                    let name = device
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown");
                    let runtime = device.get("runtime").and_then(|v| v.as_str()).unwrap_or("");
                    let udid = device.get("udid").and_then(|v| v.as_str()).unwrap_or("");
                    println!("  {} {} ({})", color::green("●"), name, runtime);
                    println!("    {}", color::dim(udid));
                }
                println!();
            }

            if !simulators.is_empty() {
                println!("Simulators:\n");
                for device in simulators.iter() {
                    let name = device
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown");
                    let runtime = device.get("runtime").and_then(|v| v.as_str()).unwrap_or("");
                    let state = device
                        .get("state")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown");
                    let udid = device.get("udid").and_then(|v| v.as_str()).unwrap_or("");
                    let state_indicator = if state == "Booted" {
                        color::green("●")
                    } else {
                        color::dim("○")
                    };
                    println!("  {} {} ({})", state_indicator, name, runtime);
                    println!("    {}", color::dim(udid));
                }
            }
            return;
        }
        // Tabs
        if let Some(tabs) = data.get("tabs").and_then(|v| v.as_array()) {
            for tab in tabs {
                let tab_id = tab.get("tabId").and_then(|v| v.as_str()).unwrap_or("?");
                let tab_label = tab.get("label").and_then(|v| v.as_str());
                let title = tab
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Untitled");
                let url = tab.get("url").and_then(|v| v.as_str()).unwrap_or("");
                let active = tab.get("active").and_then(|v| v.as_bool()).unwrap_or(false);
                let marker = if active {
                    color::cyan("→")
                } else {
                    " ".to_string()
                };
                if let Some(label) = tab_label {
                    println!("{} [{}] {} {} - {}", marker, tab_id, label, title, url);
                } else {
                    println!("{} [{}] {} - {}", marker, tab_id, title, url);
                }
            }
            return;
        }
        // Tab switch
        if action == Some("tab_switch") {
            if let Some(tab_id) = data.get("tabId").and_then(|v| v.as_str()) {
                if let Some(url) = data.get("url").and_then(|v| v.as_str()) {
                    println!(
                        "{} Switched to tab [{}] ({})",
                        color::success_indicator(),
                        tab_id,
                        url
                    );
                } else {
                    println!(
                        "{} Switched to tab [{}]",
                        color::success_indicator(),
                        tab_id
                    );
                }
                return;
            }
        }
        // New tab/window
        if let Some(tab_id) = data.get("tabId").and_then(|v| v.as_str()) {
            if let Some(total) = data.get("total").and_then(|v| v.as_i64()) {
                let label_noun = match action {
                    Some("window_new") => "Window opened",
                    _ => "Tab opened",
                };
                let tab_label = data.get("label").and_then(|v| v.as_str());
                if let Some(lbl) = tab_label {
                    println!(
                        "{} {} [{}] {} ({} total)",
                        color::success_indicator(),
                        label_noun,
                        tab_id,
                        lbl,
                        total
                    );
                } else {
                    println!(
                        "{} {} [{}] ({} total)",
                        color::success_indicator(),
                        label_noun,
                        tab_id,
                        total
                    );
                }
                return;
            }
        }
        // Console logs
        if let Some(logs) = data.get("messages").and_then(|v| v.as_array()) {
            if opts.content_boundaries {
                let mut console_output = String::new();
                for log in logs {
                    let level = log.get("type").and_then(|v| v.as_str()).unwrap_or("log");
                    let text = log.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    console_output.push_str(&format!(
                        "{} {}\n",
                        color::console_level_prefix(level),
                        text
                    ));
                }
                if console_output.ends_with('\n') {
                    console_output.pop();
                }
                print_with_boundaries(&console_output, origin, opts);
            } else {
                for log in logs {
                    let level = log.get("type").and_then(|v| v.as_str()).unwrap_or("log");
                    let text = log.get("text").and_then(|v| v.as_str()).unwrap_or("");
                    println!("{} {}", color::console_level_prefix(level), text);
                }
            }
            return;
        }
        // Errors
        if let Some(errors) = data.get("errors").and_then(|v| v.as_array()) {
            for err in errors {
                let msg = err.get("message").and_then(|v| v.as_str()).unwrap_or("");
                println!("{} {}", color::error_indicator(), msg);
            }
            return;
        }
        // Cookies
        if let Some(cookies) = data.get("cookies").and_then(|v| v.as_array()) {
            for cookie in cookies {
                let name = cookie.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let value = cookie.get("value").and_then(|v| v.as_str()).unwrap_or("");
                println!("{}={}", name, value);
            }
            return;
        }
        // Network requests
        if let Some(requests) = data.get("requests").and_then(|v| v.as_array()) {
            if requests.is_empty() {
                println!("No requests captured");
            } else {
                for req in requests {
                    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("GET");
                    let url = req.get("url").and_then(|v| v.as_str()).unwrap_or("");
                    let resource_type = req
                        .get("resourceType")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let request_id = req.get("requestId").and_then(|v| v.as_str()).unwrap_or("");
                    let status = req.get("status").and_then(|v| v.as_i64());
                    match status {
                        Some(s) => println!(
                            "[{}] {} {} ({}) {}",
                            request_id, method, url, resource_type, s
                        ),
                        None => println!("[{}] {} {} ({})", request_id, method, url, resource_type),
                    }
                }
            }
            return;
        }
        // Cleared (cookies, console, or request log)
        if let Some(cleared) = data.get("cleared").and_then(|v| v.as_bool()) {
            if cleared {
                let label = match action {
                    Some("cookies_clear") => "Cookies cleared",
                    Some("console") => "Console log cleared",
                    _ => "Request log cleared",
                };
                println!("{} {}", color::success_indicator(), label);
                return;
            }
        }
        // Bounding box
        if let Some(box_data) = data.get("box") {
            println!(
                "{}",
                serde_json::to_string_pretty(box_data).unwrap_or_default()
            );
            return;
        }
        // Element styles
        if let Some(elements) = data.get("elements").and_then(|v| v.as_array()) {
            for (i, el) in elements.iter().enumerate() {
                let tag = el.get("tag").and_then(|v| v.as_str()).unwrap_or("?");
                let text = el.get("text").and_then(|v| v.as_str()).unwrap_or("");
                println!("[{}] {} \"{}\"", i, tag, text);

                if let Some(box_data) = el.get("box") {
                    let w = box_data.get("width").and_then(|v| v.as_i64()).unwrap_or(0);
                    let h = box_data.get("height").and_then(|v| v.as_i64()).unwrap_or(0);
                    let x = box_data.get("x").and_then(|v| v.as_i64()).unwrap_or(0);
                    let y = box_data.get("y").and_then(|v| v.as_i64()).unwrap_or(0);
                    println!("    box: {}x{} at ({}, {})", w, h, x, y);
                }

                if let Some(styles) = el.get("styles") {
                    let font_size = styles
                        .get("fontSize")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let font_weight = styles
                        .get("fontWeight")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let font_family = styles
                        .get("fontFamily")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let color = styles.get("color").and_then(|v| v.as_str()).unwrap_or("");
                    let bg = styles
                        .get("backgroundColor")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let radius = styles
                        .get("borderRadius")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    println!("    font: {} {} {}", font_size, font_weight, font_family);
                    println!("    color: {}", color);
                    println!("    background: {}", bg);
                    if radius != "0px" {
                        println!("    border-radius: {}", radius);
                    }
                }
                println!();
            }
            return;
        }
        // Closed (browser or tab)
        if data.get("closed").is_some() {
            let label = match action {
                Some("tab_close") => {
                    if let Some(closed_id) = data.get("tabId").and_then(|v| v.as_str()) {
                        println!("{} Tab [{}] closed", color::success_indicator(), closed_id);
                        return;
                    }
                    "Tab closed"
                }
                _ => "Browser closed",
            };
            println!("{} {}", color::success_indicator(), label);
            return;
        }
        // Started actions (profiling, HAR, recording)
        if let Some(started) = data.get("started").and_then(|v| v.as_bool()) {
            if started {
                match action {
                    Some("profiler_start") => {
                        println!("{} Profiling started", color::success_indicator());
                    }
                    Some("har_start") => {
                        println!("{} HAR recording started", color::success_indicator());
                    }
                    _ => {
                        if let Some(path) = data.get("path").and_then(|v| v.as_str()) {
                            println!("{} Recording started: {}", color::success_indicator(), path);
                        } else {
                            println!("{} Recording started", color::success_indicator());
                        }
                    }
                }
                return;
            }
        }
        // Recording restart (has "stopped" field - from recording_restart action)
        if data.get("stopped").is_some() {
            let path = data
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            if let Some(prev_path) = data.get("previousPath").and_then(|v| v.as_str()) {
                println!(
                    "{} Recording restarted: {} (previous saved to {})",
                    color::success_indicator(),
                    path,
                    prev_path
                );
            } else {
                println!("{} Recording started: {}", color::success_indicator(), path);
            }
            return;
        }
        // Recording stop (has "frames" field - from recording_stop action)
        if data.get("frames").is_some() {
            if let Some(path) = data.get("path").and_then(|v| v.as_str()) {
                if let Some(error) = data.get("error").and_then(|v| v.as_str()) {
                    println!(
                        "{} Recording saved to {} - {}",
                        color::warning_indicator(),
                        path,
                        error
                    );
                } else {
                    println!("{} Recording saved to {}", color::success_indicator(), path);
                }
            } else {
                println!("{} Recording stopped", color::success_indicator());
            }
            return;
        }
        // Download response (has "suggestedFilename" or "filename" field)
        if data.get("suggestedFilename").is_some() || data.get("filename").is_some() {
            if let Some(path) = data.get("path").and_then(|v| v.as_str()) {
                let filename = data
                    .get("suggestedFilename")
                    .or_else(|| data.get("filename"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if filename.is_empty() {
                    println!(
                        "{} Downloaded to {}",
                        color::success_indicator(),
                        color::green(path)
                    );
                } else {
                    println!(
                        "{} Downloaded to {} ({})",
                        color::success_indicator(),
                        color::green(path),
                        filename
                    );
                }
                return;
            }
        }
        // Trace stop without path
        if data.get("traceStopped").is_some() {
            println!("{} Trace stopped", color::success_indicator());
            return;
        }
        // Path-based operations (screenshot/pdf/trace/har/download/state/video)
        if let Some(path) = data.get("path").and_then(|v| v.as_str()) {
            match action.unwrap_or("") {
                "screenshot" => {
                    println!(
                        "{} Screenshot saved to {}",
                        color::success_indicator(),
                        color::green(path)
                    );
                    if let Some(annotations) = data.get("annotations").and_then(|v| v.as_array()) {
                        for ann in annotations {
                            let num = ann.get("number").and_then(|n| n.as_u64()).unwrap_or(0);
                            let ref_id = ann.get("ref").and_then(|r| r.as_str()).unwrap_or("");
                            let role = ann.get("role").and_then(|r| r.as_str()).unwrap_or("");
                            let name = ann.get("name").and_then(|n| n.as_str()).unwrap_or("");
                            if name.is_empty() {
                                println!(
                                    "   {} @{} {}",
                                    color::dim(&format!("[{}]", num)),
                                    ref_id,
                                    role,
                                );
                            } else {
                                println!(
                                    "   {} @{} {} {:?}",
                                    color::dim(&format!("[{}]", num)),
                                    ref_id,
                                    role,
                                    name,
                                );
                            }
                        }
                    }
                }
                "pdf" => println!(
                    "{} PDF saved to {}",
                    color::success_indicator(),
                    color::green(path)
                ),
                "trace_stop" => println!(
                    "{} Trace saved to {}",
                    color::success_indicator(),
                    color::green(path)
                ),
                "profiler_stop" => println!(
                    "{} Profile saved to {} ({} events)",
                    color::success_indicator(),
                    color::green(path),
                    data.get("eventCount").and_then(|c| c.as_u64()).unwrap_or(0)
                ),
                "har_stop" => println!(
                    "{} HAR saved to {} ({} requests)",
                    color::success_indicator(),
                    color::green(path),
                    data.get("requestCount")
                        .and_then(|c| c.as_u64())
                        .unwrap_or(0)
                ),
                "download" | "waitfordownload" => println!(
                    "{} Download saved to {}",
                    color::success_indicator(),
                    color::green(path)
                ),
                "video_stop" => println!(
                    "{} Video saved to {}",
                    color::success_indicator(),
                    color::green(path)
                ),
                "state_save" => println!(
                    "{} State saved to {}",
                    color::success_indicator(),
                    color::green(path)
                ),
                "state_load" => {
                    if let Some(note) = data.get("note").and_then(|v| v.as_str()) {
                        println!("{}", note);
                    }
                    println!(
                        "{} State path set to {}",
                        color::success_indicator(),
                        color::green(path)
                    );
                }
                // video_start and other commands that provide a path with a note
                "video_start" => {
                    if let Some(note) = data.get("note").and_then(|v| v.as_str()) {
                        println!("{}", note);
                    }
                    println!("Path: {}", path);
                }
                _ => println!(
                    "{} Saved to {}",
                    color::success_indicator(),
                    color::green(path)
                ),
            }
            return;
        }

        // State list
        if let Some(files) = data.get("files").and_then(|v| v.as_array()) {
            if let Some(dir) = data.get("directory").and_then(|v| v.as_str()) {
                println!("{}", color::bold(&format!("Saved states in {}", dir)));
            }
            if files.is_empty() {
                println!("{}", color::dim("  No state files found"));
            } else {
                for file in files {
                    let filename = file.get("filename").and_then(|v| v.as_str()).unwrap_or("");
                    let size = file.get("size").and_then(|v| v.as_i64()).unwrap_or(0);
                    let modified = file.get("modified").and_then(|v| v.as_str()).unwrap_or("");
                    let encrypted = file
                        .get("encrypted")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let size_str = if size > 1024 {
                        format!("{:.1}KB", size as f64 / 1024.0)
                    } else {
                        format!("{}B", size)
                    };
                    let date_str = modified.split('T').next().unwrap_or(modified);
                    let enc_str = if encrypted { " [encrypted]" } else { "" };
                    println!(
                        "  {} {}",
                        filename,
                        color::dim(&format!("({}, {}){}", size_str, date_str, enc_str))
                    );
                }
            }
            return;
        }

        // State rename
        if let Some(true) = data.get("renamed").and_then(|v| v.as_bool()) {
            let old_name = data.get("oldName").and_then(|v| v.as_str()).unwrap_or("");
            let new_name = data.get("newName").and_then(|v| v.as_str()).unwrap_or("");
            println!(
                "{} Renamed {} -> {}",
                color::success_indicator(),
                old_name,
                new_name
            );
            return;
        }

        // State clear
        if let Some(cleared) = data.get("cleared").and_then(|v| v.as_i64()) {
            println!(
                "{} Cleared {} state file(s)",
                color::success_indicator(),
                cleared
            );
            return;
        }

        // State show summary
        if let Some(summary) = data.get("summary") {
            let cookies = summary.get("cookies").and_then(|v| v.as_i64()).unwrap_or(0);
            let origins = summary.get("origins").and_then(|v| v.as_i64()).unwrap_or(0);
            let encrypted = data
                .get("encrypted")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let enc_str = if encrypted { " (encrypted)" } else { "" };
            println!("State file summary{}:", enc_str);
            println!("  Cookies: {}", cookies);
            println!("  Origins with localStorage: {}", origins);
            return;
        }

        // State clean
        if let Some(cleaned) = data.get("cleaned").and_then(|v| v.as_i64()) {
            println!(
                "{} Cleaned {} old state file(s)",
                color::success_indicator(),
                cleaned
            );
            return;
        }

        // Informational note
        if let Some(note) = data.get("note").and_then(|v| v.as_str()) {
            println!("{}", note);
            return;
        }
        // Auth list
        if let Some(profiles) = data.get("profiles").and_then(|v| v.as_array()) {
            if profiles.is_empty() {
                println!("{}", color::dim("No auth profiles saved"));
            } else {
                println!("{}", color::bold("Auth profiles:"));
                for p in profiles {
                    let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let url = p.get("url").and_then(|v| v.as_str()).unwrap_or("");
                    let user = p.get("username").and_then(|v| v.as_str()).unwrap_or("");
                    println!(
                        "  {} {} {}",
                        color::green(name),
                        color::dim(user),
                        color::dim(url)
                    );
                }
            }
            return;
        }

        // Auth show
        if let Some(profile) = data.get("profile").and_then(|v| v.as_object()) {
            let name = profile.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let url = profile.get("url").and_then(|v| v.as_str()).unwrap_or("");
            let user = profile
                .get("username")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let created = profile
                .get("createdAt")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let last_login = profile.get("lastLoginAt").and_then(|v| v.as_str());
            println!("Name: {}", name);
            println!("URL: {}", url);
            println!("Username: {}", user);
            println!("Created: {}", created);
            if let Some(ll) = last_login {
                println!("Last login: {}", ll);
            }
            return;
        }

        // Auth save/update/login/delete
        if data.get("saved").and_then(|v| v.as_bool()).unwrap_or(false) {
            let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("");
            println!(
                "{} Auth profile '{}' saved",
                color::success_indicator(),
                name
            );
            return;
        }
        if data
            .get("updated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            && !data.get("saved").and_then(|v| v.as_bool()).unwrap_or(false)
        {
            let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("");
            println!(
                "{} Auth profile '{}' updated",
                color::success_indicator(),
                name
            );
            return;
        }
        if data
            .get("loggedIn")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(title) = data.get("title").and_then(|v| v.as_str()) {
                println!(
                    "{} Logged in as '{}' - {}",
                    color::success_indicator(),
                    name,
                    title
                );
            } else {
                println!("{} Logged in as '{}'", color::success_indicator(), name);
            }
            return;
        }
        if data
            .get("deleted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            if let Some(name) = data.get("name").and_then(|v| v.as_str()) {
                println!(
                    "{} Auth profile '{}' deleted",
                    color::success_indicator(),
                    name
                );
                return;
            }
        }

        // Confirmation required (for orchestrator use)
        if let Some(pending) = confirmation_data(data) {
            print_confirmation_required(pending);
            return;
        }
        if data
            .get("confirmed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            println!("{} Action confirmed", color::success_indicator());
            return;
        }
        if data
            .get("denied")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            println!("{} Action denied", color::success_indicator());
            return;
        }

        // Default success
        println!("{} Done", color::success_indicator());
    }

    print_warning(resp);
}

fn print_lifecycle_note(data: &serde_json::Value) {
    let Some(lifecycle) = data.get("lifecycle") else {
        return;
    };

    let mut parts: Vec<String> = Vec::new();
    let relaunched = lifecycle
        .get("relaunchedBrowser")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let launched = lifecycle
        .get("launched")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let reused = lifecycle
        .get("reused")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if relaunched {
        parts.push("relaunched browser".to_string());
    } else if launched && !reused {
        parts.push("launched browser".to_string());
    }

    if let Some(status) = lifecycle.get("restoreStatus").and_then(|v| v.as_str()) {
        if !matches!(status, "not_configured" | "pending") {
            parts.push(format!("restore: {}", status));
        }
    }

    if let Some(status) = lifecycle.get("saveStatus").and_then(|v| v.as_str()) {
        if !matches!(status, "not_attempted" | "not_configured") {
            parts.push(format!("save: {}", status));
        }
    }

    if !parts.is_empty() {
        eprintln!("{} {}", color::dim("[a3s use browser]"), parts.join("; "));
    }
}

fn print_warning(resp: &Response) {
    if let Some(ref warning) = resp.warning {
        eprintln!("{} {}", color::warning_indicator(), warning);
    }
}

/// Print command-specific help. Returns true if help was printed, false if command unknown.
pub fn print_command_help(command: &str) -> bool {
    let help = match command {
        // === Navigation ===
        "open" | "goto" | "navigate" => {
            r##"
a3s use browser open - Launch the browser, optionally navigate

Usage: a3s use browser open [url]

Without a URL, launches the browser but stays on about:blank. This lets
you stage state (network routes, cookies, init scripts) before the first
real navigation — useful for SSR debug, auth setup, and capturing fresh
`react suspense` / `vitals` state without noise from a prior page.

With a URL, launches and navigates. If no protocol is provided, https://
is automatically prepended.

The `goto` and `navigate` aliases still require a URL.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session
  --headers <json>     Set HTTP headers (scoped to this origin)
  --headed             Show browser window
  --enable react-devtools   Inject the React DevTools hook before any page JS
  --init-script <path>      Register a page init script (repeatable)

Examples:
  a3s use browser open                     # Launch, no nav
  a3s use browser open example.com
  a3s use browser open https://github.com
  a3s use browser open localhost:3000
  a3s use browser open api.example.com --headers '{"Authorization": "Bearer token"}'
    # ^ Headers only sent to api.example.com, not other domains

  # Pre-navigation setup in one turn:
  a3s use browser batch \
    '["open"]' \
    '["network","route","*","--abort","--resource-type","script"]' \
    '["navigate","http://localhost:3000/target"]'
"##
        }
        "back" => {
            r##"
a3s use browser back - Navigate back in history

Usage: a3s use browser back

Goes back one page in the browser history, equivalent to clicking
the browser's back button.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser back
"##
        }
        "forward" => {
            r##"
a3s use browser forward - Navigate forward in history

Usage: a3s use browser forward

Goes forward one page in the browser history, equivalent to clicking
the browser's forward button.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser forward
"##
        }
        "reload" => {
            r##"
a3s use browser reload - Reload the current page

Usage: a3s use browser reload

Reloads the current page, equivalent to pressing F5 or clicking
the browser's reload button.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser reload
"##
        }

        "read" => {
            r##"
a3s use browser read - Fetch a URL as agent-readable text

Usage: a3s use browser read [url] [--raw] [--require-md] [--llms <index|full>] [--outline] [--filter <text>] [--timeout <ms>]

Fetches a URL as agent-readable text. Omit the URL to read the rendered DOM of
the active tab in the current browser session. Explicit URL reads prefer
markdown with Accept: text/markdown, try the same URL with .md appended when
the first response is not markdown, walk ancestor paths toward / to find the
nearest llms.txt for a matching docs link, fall back to plain text or readable
text extracted from HTML, and print only the document content by default.
Use --outline for a compact heading outline of a single page. Use --llms index
or --llms full for nearest-ancestor llms files; with no URL, --llms and
--require-md use the active tab URL because they depend on HTTP resources.

Options:
  --raw                Print the response body without HTML extraction
  --require-md         Fail unless the response is Content-Type: text/markdown
  --llms <index|full>  Print nearest llms.txt links or llms-full.txt
  --outline            Print a heading outline for the selected page
  --filter <text>      Filter page sections, --llms links/sections, or --outline headings
  --timeout <ms>       Request timeout in milliseconds (default: 10000)

Global Options:
  --json               Output metadata and content as JSON
  --headers <json>     Additional HTTP headers, such as Authorization
  --allowed-domains <list>  Restrict read fetches and redirects to allowed domains
  --content-boundaries Wrap read output in boundary markers
  --max-output <chars> Truncate read output to N chars

Examples:
  a3s use browser read
  a3s use browser read https://docs.example.com/guide
  a3s use browser read https://docs.example.com/guide --filter auth
  a3s use browser read https://docs.example.com/guide --outline
  a3s use browser read https://docs.example.com --llms index --filter auth
  a3s use browser read https://docs.example.com --llms full --filter auth
  a3s use browser read docs.example.com/guide --require-md
  a3s use browser read https://api.example.com/docs --headers '{"Authorization":"Bearer token"}'
"##
        }

        // === Core Actions ===
        "click" => {
            r##"
a3s use browser click - Click an element

Usage: a3s use browser click <selector> [--new-tab]

Clicks on the specified element. The selector can be a CSS selector,
XPath, or an element reference from snapshot (e.g., @e1).

If another element covers the click point, a3s use browser reports the
covering element instead of dispatching a click to the wrong target.

Options:
  --new-tab            Open link in a new tab instead of navigating current tab
                       (only works on elements with href attribute)

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser click "#submit-button"
  a3s use browser click @e1
  a3s use browser click "button.primary"
  a3s use browser click "//button[@type='submit']"
  a3s use browser click @e3 --new-tab
"##
        }
        "dblclick" => {
            r##"
a3s use browser dblclick - Double-click an element

Usage: a3s use browser dblclick <selector>

Double-clicks on the specified element. Useful for text selection
or triggering double-click handlers.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser dblclick "#editable-text"
  a3s use browser dblclick @e5
"##
        }
        "fill" => {
            r##"
a3s use browser fill - Clear and fill an input field

Usage: a3s use browser fill <selector> <text>

Clears the input field and fills it with the specified text.
This replaces any existing content in the field.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser fill "#email" "user@example.com"
  a3s use browser fill @e3 "Hello World"
  a3s use browser fill "input[name='search']" "query"
"##
        }
        "type" => {
            r##"
a3s use browser type - Type text into an element

Usage: a3s use browser type <selector> <text>

Types text into the specified element character by character.
Unlike fill, this does not clear existing content first.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser type "#search" "hello"
  a3s use browser type @e2 "additional text"

See Also:
  For typing into contenteditable editors (Lexical, ProseMirror, etc.)
  without a selector, use 'keyboard type' instead:
    a3s use browser keyboard type "# My Heading"
"##
        }
        "hover" => {
            r##"
a3s use browser hover - Hover over an element

Usage: a3s use browser hover <selector>

Moves the mouse to hover over the specified element. Useful for
triggering hover states or dropdown menus.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser hover "#dropdown-trigger"
  a3s use browser hover @e4
"##
        }
        "focus" => {
            r##"
a3s use browser focus - Focus an element

Usage: a3s use browser focus <selector>

Sets keyboard focus to the specified element.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser focus "#input-field"
  a3s use browser focus @e2
"##
        }
        "check" => {
            r##"
a3s use browser check - Check a checkbox

Usage: a3s use browser check <selector>

Checks a checkbox element. If already checked, no action is taken.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser check "#terms-checkbox"
  a3s use browser check @e7
"##
        }
        "uncheck" => {
            r##"
a3s use browser uncheck - Uncheck a checkbox

Usage: a3s use browser uncheck <selector>

Unchecks a checkbox element. If already unchecked, no action is taken.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser uncheck "#newsletter-opt-in"
  a3s use browser uncheck @e8
"##
        }
        "select" => {
            r##"
a3s use browser select - Select a dropdown option

Usage: a3s use browser select <selector> <value...>

Selects one or more options in a <select> dropdown by value.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser select "#country" "US"
  a3s use browser select @e5 "option2"
  a3s use browser select "#menu" "opt1" "opt2" "opt3"
"##
        }
        "drag" => {
            r##"
a3s use browser drag - Drag and drop

Usage: a3s use browser drag <source> <target>

Drags an element from source to target location.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser drag "#draggable" "#drop-zone"
  a3s use browser drag @e1 @e2
"##
        }
        "upload" => {
            r##"
a3s use browser upload - Upload files

Usage: a3s use browser upload <selector> <files...>

Uploads one or more files to a file input element.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser upload "#file-input" ./document.pdf
  a3s use browser upload @e3 ./image1.png ./image2.png
"##
        }
        "download" => {
            r##"
a3s use browser download - Download a file by clicking an element

Usage: a3s use browser download <selector> <path>

Clicks an element that triggers a download and saves the file to the specified path.

Arguments:
  selector             Element to click (CSS selector or @ref)
  path                 Path where the downloaded file will be saved

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser download "#download-btn" ./file.pdf
  a3s use browser download @e5 ./report.xlsx
  a3s use browser download "a[href$='.zip']" ./archive.zip
"##
        }

        // === Keyboard ===
        "press" | "key" => {
            r##"
a3s use browser press - Press a key or key combination

Usage: a3s use browser press <key>

Presses a key or key combination. Supports special keys and modifiers.

Aliases: key

Special Keys:
  Enter, Tab, Escape, Backspace, Delete, Space
  ArrowUp, ArrowDown, ArrowLeft, ArrowRight
  Home, End, PageUp, PageDown
  F1-F12

Modifiers (combine with +):
  Control, Alt, Shift, Meta

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser press Enter
  a3s use browser press Tab
  a3s use browser press Control+a
  a3s use browser press Control+Shift+s
  a3s use browser press Escape
"##
        }
        "keydown" => {
            r##"
a3s use browser keydown - Press a key down (without release)

Usage: a3s use browser keydown <key>

Presses a key down without releasing it. Use keyup to release.
Useful for holding modifier keys.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser keydown Shift
  a3s use browser keydown Control
"##
        }
        "keyup" => {
            r##"
a3s use browser keyup - Release a key

Usage: a3s use browser keyup <key>

Releases a key that was pressed with keydown.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser keyup Shift
  a3s use browser keyup Control
"##
        }
        "keyboard" => {
            r##"
a3s use browser keyboard - Raw keyboard input (no selector needed)

Usage: a3s use browser keyboard <subcommand> <text>

Sends keyboard input to whatever element currently has focus.
Unlike 'type' which requires a selector, 'keyboard' operates on
the current focus — essential for contenteditable editors like
Lexical, ProseMirror, CodeMirror, and Monaco.

Subcommands:
  type <text>          Type text character-by-character with real
                       key events (keydown, keypress, keyup per char)
  inserttext <text>    Insert text without key events (like paste)

Note: For key combos (Enter, Control+a), use the 'press' command
directly — it already operates on the current focus.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser keyboard type "Hello, World!"
  a3s use browser keyboard type "# My Heading"
  a3s use browser keyboard inserttext "pasted content"

Use Cases:
  # Type into a Lexical/ProseMirror contenteditable editor:
  a3s use browser click "[contenteditable]"
  a3s use browser keyboard type "# My Heading"
  a3s use browser press Enter
  a3s use browser keyboard type "Some paragraph text"
"##
        }

        // === Scroll ===
        "scroll" => {
            r##"
a3s use browser scroll - Scroll the page

Usage: a3s use browser scroll [direction] [amount] [options]

Scrolls the page or a specific element in the specified direction.

Arguments:
  direction            up, down, left, right (default: down)
  amount               Pixels to scroll (default: 300)

Options:
  -s, --selector <sel> CSS selector for a scrollable container

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser scroll
  a3s use browser scroll down 500
  a3s use browser scroll up 200
  a3s use browser scroll left 100
  a3s use browser scroll down 500 --selector "div.scroll-container"
"##
        }
        "scrollintoview" | "scrollinto" => {
            r##"
a3s use browser scrollintoview - Scroll element into view

Usage: a3s use browser scrollintoview <selector>

Scrolls the page until the specified element is visible in the viewport.

Aliases: scrollinto

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser scrollintoview "#footer"
  a3s use browser scrollintoview @e15
"##
        }

        // === Wait ===
        "wait" => {
            r##"
a3s use browser wait - Wait for condition

Usage: a3s use browser wait <selector|ms|option>

Waits for an element to appear, a timeout, or other conditions.

Modes:
  <selector>           Wait for element to appear
  <ms>                 Wait for specified milliseconds
  --url <pattern>      Wait for URL to match pattern
  --load <state>       Wait for load state (load, domcontentloaded, networkidle)
  --fn <expression>    Wait for JavaScript expression to be truthy
  --text <text>        Wait for text to appear on page (substring match)
  --download [path]    Wait for a download to complete (optionally save to path)

Download Options (with --download):
  --timeout <ms>       Timeout in milliseconds for download to start

Wait for text to disappear:
  Use --fn or --state hidden to wait for text or elements to go away:
  wait --fn "!document.body.innerText.includes('Loading...')"
  wait "#spinner" --state hidden
  wait @e5 --state detached

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser wait "#loading-spinner"
  a3s use browser wait 2000
  a3s use browser wait --url "**/dashboard"
  a3s use browser wait --load networkidle
  a3s use browser wait --fn "window.appReady === true"
  a3s use browser wait --text "Welcome back"
  a3s use browser wait --download ./file.pdf
  a3s use browser wait --download ./report.xlsx --timeout 30000
  a3s use browser wait --fn "!document.body.innerText.includes('Loading...')"
"##
        }

        // === Screenshot/PDF ===
        "screenshot" => {
            r##"
a3s use browser screenshot - Take a screenshot

Usage: a3s use browser screenshot [selector] [path]

Captures a screenshot of the current page. If no path is provided,
saves to a temporary directory with a generated filename.
Headless Chromium screenshots hide native scrollbars for consistent image output.
Pass --hide-scrollbars false when launching to keep native scrollbars visible.

Options:
  --full, -f           Capture full page (not just viewport)
  --annotate           Overlay numbered labels on interactive elements.
                       Each label [N] corresponds to ref @eN from snapshot.
                       Prints a legend mapping labels to element roles/names.
                       With --json, annotations are included in the response.
                       Supported on Chromium and Lightpanda.
  --screenshot-dir <path>  Default output directory for screenshots
                       (or A3S_USE_BROWSER_SCREENSHOT_DIR env)
  --screenshot-quality <0-100>  JPEG quality (0-100, only applies to jpeg format)
                       (or A3S_USE_BROWSER_SCREENSHOT_QUALITY env)
  --screenshot-format <fmt>  Image format: png (default) or jpeg
                       (or A3S_USE_BROWSER_SCREENSHOT_FORMAT env)

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser screenshot
  a3s use browser screenshot ./screenshot.png
  a3s use browser screenshot --full ./full-page.png
  a3s use browser screenshot --annotate              # Labeled screenshot + legend
  a3s use browser screenshot --annotate ./page.png   # Save annotated screenshot
  a3s use browser screenshot --annotate --json       # JSON output with annotations
  a3s use browser screenshot --screenshot-dir ./shots # Save to custom directory
  a3s use browser screenshot --screenshot-format jpeg --screenshot-quality 80
"##
        }
        "pdf" => {
            r##"
a3s use browser pdf - Save page as PDF

Usage: a3s use browser pdf <path>

Saves the current page as a PDF file.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser pdf ./page.pdf
  a3s use browser pdf ~/Documents/report.pdf
"##
        }

        // === Snapshot ===
        "snapshot" => {
            r##"
a3s use browser snapshot - Get accessibility tree snapshot

Usage: a3s use browser snapshot [options]

Returns an accessibility tree representation of the page with element
references (like @e1, @e2) that can be used in subsequent commands.
Designed for AI agents to understand page structure.

Options:
  -i, --interactive    Only include interactive elements
  -u, --urls           Include href URLs for link elements
  -c, --compact        Remove empty structural elements
  -d, --depth <n>      Limit tree depth
  -s, --selector <sel> Scope snapshot to CSS selector

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser snapshot
  a3s use browser snapshot -i
  a3s use browser snapshot -i --urls
  a3s use browser snapshot --compact --depth 5
  a3s use browser snapshot -s "#main-content"
"##
        }

        // === Eval ===
        "eval" => {
            r##"
a3s use browser eval - Execute JavaScript

Usage: a3s use browser eval [options] <script>

Executes JavaScript code in the browser context and returns the result.

Options:
  -b, --base64         Decode script from base64 (avoids shell escaping issues)
  --stdin              Read script from stdin (useful for heredocs/multiline)

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser eval "document.title"
  a3s use browser eval "window.location.href"
  a3s use browser eval "document.querySelectorAll('a').length"
  a3s use browser eval -b "ZG9jdW1lbnQudGl0bGU="

  # Read from stdin with heredoc
  cat <<'EOF' | a3s use browser eval --stdin
  const links = document.querySelectorAll('a');
  links.length;
  EOF
"##
        }

        // === Close ===
        "close" | "quit" | "exit" => {
            r##"
a3s use browser close - Close the browser

Usage: a3s use browser close [options]

Closes the browser instance for the current session.

Aliases: quit, exit

Options:
  --all                Close all active sessions

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser close
  a3s use browser close --session mysession
  a3s use browser close --all
"##
        }

        // === Inspect ===
        "inspect" => {
            r##"
a3s use browser inspect - Open Chrome DevTools for the active page

Starts a local WebSocket proxy and opens Chrome's DevTools frontend in your
default browser. The proxy routes DevTools traffic through the daemon's
existing CDP connection, so both DevTools and a3s use browser commands work
simultaneously.

Usage: a3s use browser inspect

Examples:
  a3s use browser open example.com
  a3s use browser inspect          # opens DevTools in your browser
  a3s use browser click "Submit"   # commands still work while DevTools is open
"##
        }

        // === Get ===
        "get" => {
            r##"
a3s use browser get - Retrieve information from elements or page

Usage: a3s use browser get <subcommand> [args]

Retrieves various types of information from elements or the page.

Subcommands:
  text <selector>            Get text content of element
  html <selector>            Get inner HTML of element
  value <selector>           Get value of input element
  attr <selector> <name>     Get attribute value
  title                      Get page title
  url                        Get current URL
  count <selector>           Count matching elements
  box <selector>             Get bounding box (x, y, width, height)
  styles <selector>          Get computed styles of elements
  cdp-url                    Get Chrome DevTools Protocol WebSocket URL

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser get text @e1
  a3s use browser get html "#content"
  a3s use browser get value "#email-input"
  a3s use browser get attr "#link" href
  a3s use browser get title
  a3s use browser get url
  a3s use browser get count "li.item"
  a3s use browser get box "#header"
  a3s use browser get styles "button"
  a3s use browser get styles @e1
"##
        }

        // === Is ===
        "is" => {
            r##"
a3s use browser is - Check element state

Usage: a3s use browser is <subcommand> <selector>

Checks the state of an element and returns true/false.

Subcommands:
  visible <selector>   Check if element is visible
  enabled <selector>   Check if element is enabled (not disabled)
  checked <selector>   Check if checkbox/radio is checked

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser is visible "#modal"
  a3s use browser is enabled "#submit-btn"
  a3s use browser is checked "#agree-checkbox"
"##
        }

        // === Find ===
        "find" => {
            r##"
a3s use browser find - Find and interact with elements by locator

Usage: a3s use browser find <locator> <value> [action] [text]

Finds elements using semantic locators and optionally performs an action.

Locators:
  role <role>              Find by ARIA role (--name <n>, --exact)
  text <text>              Find by text content (--exact)
  label <label>            Find by associated label (--exact)
  placeholder <text>       Find by placeholder text (--exact)
  alt <text>               Find by alt text (--exact)
  title <text>             Find by title attribute (--exact)
  testid <id>              Find by data-testid attribute
  first <selector>         First matching element
  last <selector>          Last matching element
  nth <index> <selector>   Nth matching element (0-based)

Actions (default: click):
  click, fill, type, hover, focus, check, uncheck

Options:
  --name <name>        Filter role by accessible name
  --exact              Require exact text match

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser find role button click --name Submit
  a3s use browser find text "Sign In" click
  a3s use browser find label "Email" fill "user@example.com"
  a3s use browser find placeholder "Search..." type "query"
  a3s use browser find testid "login-form" click
  a3s use browser find first "li.item" click
  a3s use browser find nth 2 ".card" hover
"##
        }

        // === Mouse ===
        "mouse" => {
            r##"
a3s use browser mouse - Low-level mouse operations

Usage: a3s use browser mouse <subcommand> [args]

Performs low-level mouse operations for precise control.

Subcommands:
  move <x> <y>         Move mouse to coordinates
  down [button]        Press mouse button (left, right, middle)
  up [button]          Release mouse button
  wheel <dy> [dx]      Scroll mouse wheel

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser mouse move 100 200
  a3s use browser mouse down
  a3s use browser mouse up
  a3s use browser mouse down right
  a3s use browser mouse wheel 100
  a3s use browser mouse wheel -50 0
"##
        }

        // === Set ===
        "set" => {
            r##"
a3s use browser set - Configure browser settings

Usage: a3s use browser set <setting> [args]

Configures various browser settings and emulation options.

Settings:
  viewport <w> <h> [scale]   Set viewport size (scale = deviceScaleFactor, e.g. 2 for retina)
  device <name>              Emulate device (e.g., "iPhone 12")
  geo <lat> <lng>            Set geolocation
  offline [on|off]           Toggle offline mode
  headers <json>             Set extra HTTP headers
  credentials <user> <pass>  Set HTTP authentication
  media [dark|light]         Set color scheme preference
        [reduced-motion]     Enable reduced motion

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser set viewport 1920 1080
  a3s use browser set viewport 1920 1080 2    # 2x retina
  a3s use browser set device "iPhone 12"
  a3s use browser set geo 37.7749 -122.4194
  a3s use browser set offline on
  a3s use browser set headers '{"X-Custom": "value"}'
  a3s use browser set credentials admin secret123
  a3s use browser set media dark
  a3s use browser set media light reduced-motion
"##
        }

        // === Network ===
        "network" => {
            r##"
a3s use browser network - Network interception and monitoring

Usage: a3s use browser network <subcommand> [args]

Intercept, mock, or monitor network requests.

Subcommands:
  route <url> [options]      Intercept requests matching URL pattern
    --abort                  Abort matching requests
    --body <json>            Respond with custom body
  unroute [url]              Remove route (all if no URL)
  requests [options]         List captured requests
    --clear                  Clear request log
    --filter <pattern>       Filter by URL pattern
    --type <types>           Filter by resource type (comma-separated: xhr,fetch,document)
    --method <method>        Filter by HTTP method (GET, POST, etc.)
    --status <code>          Filter by status (200, 2xx, 400-499)
  request <requestId>        View full request/response detail (including body)
  har <start|stop> [path]    Record and export a HAR file

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser network route "**/api/*" --abort
  a3s use browser network route "**/data.json" --body '{"mock": true}'
  a3s use browser network unroute
  a3s use browser network requests
  a3s use browser network requests --filter "api"
  a3s use browser network requests --type xhr,fetch
  a3s use browser network requests --method POST --status 2xx
  a3s use browser network requests --clear
  a3s use browser network request 1234.5
  a3s use browser network har start
  a3s use browser network har stop ./capture.har
"##
        }

        // === Storage ===
        "storage" => {
            r##"
a3s use browser storage - Manage web storage

Usage: a3s use browser storage <type> [operation] [key] [value]

Manage localStorage and sessionStorage.

Types:
  local                localStorage
  session              sessionStorage

Operations:
  get [key]            Get all storage or specific key
  set <key> <value>    Set a key-value pair
  clear                Clear all storage

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser storage local
  a3s use browser storage local get authToken
  a3s use browser storage local set theme "dark"
  a3s use browser storage local clear
  a3s use browser storage session get userId
"##
        }

        // === Cookies ===
        "cookies" => {
            r##"
a3s use browser cookies - Manage browser cookies

Usage: a3s use browser cookies [operation] [args]

Manage browser cookies for the current context.

Operations:
  get                                Get all cookies (default)
  set <name> <value> [options]       Set a cookie with optional properties
  clear                              Clear all cookies

Cookie Set Options:
  --url <url>                        URL for the cookie (allows setting before page load)
  --domain <domain>                  Cookie domain (e.g., ".example.com")
  --path <path>                      Cookie path (e.g., "/api")
  --httpOnly                         Set HttpOnly flag (prevents JavaScript access)
  --secure                           Set Secure flag (HTTPS only)
  --sameSite <Strict|Lax|None>       SameSite policy
  --expires <timestamp>              Expiration time (Unix timestamp in seconds)

Note: If --url, --domain, and --path are all omitted, the cookie will be set
for the current page URL.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  # Simple cookie for current page
  a3s use browser cookies set session_id "abc123"

  # Set cookie for a URL before loading it (useful for authentication)
  a3s use browser cookies set session_id "abc123" --url https://app.example.com

  # Set secure, httpOnly cookie with domain and path
  a3s use browser cookies set auth_token "xyz789" --domain example.com --path /api --httpOnly --secure

  # Set cookie with SameSite policy
  a3s use browser cookies set tracking_consent "yes" --sameSite Strict

  # Set cookie with expiration (Unix timestamp)
  a3s use browser cookies set temp_token "temp123" --expires 1735689600

  # Get all cookies
  a3s use browser cookies

  # Clear all cookies
  a3s use browser cookies clear
"##
        }

        // === Tabs ===
        "tab" => {
            r##"
a3s use browser tab - Manage browser tabs

Usage: a3s use browser tab [operation] [args]

Manage browser tabs in the current window. Stable tab ids look like `t1`,
`t2`, `t3`. An id is never reused within a session, so scripts can keep
referring to the same tab across commands. Optional user-assigned labels
(e.g. `docs`, `app`) are interchangeable with ids everywhere a tab ref is
accepted.

Operations:
  list                       List open tabs with their ids and labels (default)
  new [url]                  Open a new tab
  new --label <name> [url]   Open a new tab with a label like `docs` or `app`
  close [t<N>|label]         Close a tab (current if no ref given)
  <t<N>|label>               Switch to a tab by id or label

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser tab
  a3s use browser tab list
  a3s use browser tab new
  a3s use browser tab new https://example.com
  a3s use browser tab new --label docs https://docs.example.com
  a3s use browser tab t2
  a3s use browser tab docs
  a3s use browser tab close
  a3s use browser tab close t1
  a3s use browser tab close docs
"##
        }

        // === Window ===
        "window" => {
            r##"
a3s use browser window - Manage browser windows

Usage: a3s use browser window <operation>

Manage browser windows.

Operations:
  new                  Open new browser window

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser window new
"##
        }

        // === Frame ===
        "frame" => {
            r##"
a3s use browser frame - Switch frame context

Usage: a3s use browser frame <selector|main>

Switch to an iframe or back to the main frame.

Arguments:
  <selector>           CSS selector for iframe
  main                 Switch back to main frame

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser frame "#embed-iframe"
  a3s use browser frame "iframe[name='content']"
  a3s use browser frame main
"##
        }

        // === Auth ===
        "auth" => {
            r##"
a3s use browser auth - Manage authentication profiles

Usage: a3s use browser auth <subcommand> [args]

Subcommands:
  save <name>              Save credentials for a login profile
  login <name>             Login using saved credentials (waits for form fields)
  list                     List saved profiles (names and URLs only)
  show <name>              Show profile metadata (no passwords)
  delete <name>            Delete a saved profile

Save Options:
  --url <url>              Login page URL (required)
  --username <user>        Username (required)
  --password <pass>        Password (required unless --password-stdin)
  --password-stdin          Read password from stdin (recommended)
  --username-selector <s>  Custom CSS selector for username field
  --password-selector <s>  Custom CSS selector for password field
  --submit-selector <s>    Custom CSS selector for submit button

Plugin Login Options:
  --credential-provider <p> Resolve credentials from configured plugin <p>
  --item <ref>              Provider-specific vault item reference
  --url <url>               Login URL override
  --username-selector <s>   Username selector override for this login
  --password-selector <s>   Password selector override for this login
  --submit-selector <s>     Submit selector override for this login

Login behavior:
  auth login waits for form selectors to appear before filling/clicking.
  Selector wait timeout follows the default action timeout.
  Plugin credentials are resolved just-in-time and are not saved locally.

Global Options:
  --json                   Output as JSON
  --session <name>         Use specific session

Examples:
  echo "pass" | a3s use browser auth save github --url https://github.com/login --username user --password-stdin
  a3s use browser auth save github --url https://github.com/login --username user --password pass
  a3s use browser auth login github
  a3s use browser auth login my-app --credential-provider vault --item "My App"
  a3s use browser auth list
  a3s use browser auth show github
  a3s use browser auth delete github
"##
        }

        // === Confirm/Deny ===
        "confirm" | "deny" => {
            r##"
a3s use browser confirm/deny - Approve or deny pending actions

Usage:
  a3s use browser confirm <confirmation-id>
  a3s use browser deny <confirmation-id>

When --confirm-actions is set, certain action categories return a
confirmation_required response with a confirmation ID. Use confirm/deny
to approve or reject the action.

Pending confirmations auto-deny after 60 seconds.

Examples:
  a3s use browser confirm c_8f3a1234
  a3s use browser deny c_8f3a1234
"##
        }

        // === Dialog ===
        "dialog" => {
            r##"
a3s use browser dialog - Handle browser dialogs

Usage: a3s use browser dialog <accept|dismiss|status> [text]

Respond to or check for browser dialogs (alert, confirm, prompt).

Operations:
  accept [text]        Accept dialog, optionally with prompt text
  dismiss              Dismiss/cancel dialog
  status               Check if a dialog is currently open

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser dialog accept
  a3s use browser dialog accept "my input"
  a3s use browser dialog dismiss
  a3s use browser dialog status
"##
        }

        // === Trace ===
        "trace" => {
            r##"
a3s use browser trace - Record execution trace

Usage: a3s use browser trace start
       a3s use browser trace stop [path]

Record a Chrome DevTools trace for debugging.

Operations:
  start                Start recording trace
  stop [path]          Stop recording and save trace

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser trace start
  a3s use browser trace stop
  a3s use browser trace stop ./debug-trace.json
"##
        }

        // === Profile (CDP Tracing) ===
        "profiler" => {
            r##"
a3s use browser profiler - Record Chrome DevTools performance profile

Usage: a3s use browser profiler <operation> [options]

Record a performance profile using Chrome DevTools Protocol (CDP) Tracing.
The output JSON file can be loaded into Chrome DevTools Performance panel,
Perfetto UI (https://ui.perfetto.dev/), or other trace analysis tools.

Operations:
  start                Start profiling
  stop [path]          Stop profiling and save to file

Start Options:
  --categories <list>  Comma-separated trace categories (default includes
                       devtools.timeline, v8.execute, blink, and others)

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  # Basic profiling
  a3s use browser profiler start
  a3s use browser navigate https://example.com
  a3s use browser click "#button"
  a3s use browser profiler stop ./trace.json

  # With custom categories
  a3s use browser profiler start --categories "devtools.timeline,v8.execute,blink.user_timing"
  a3s use browser profiler stop ./custom-trace.json

The output file can be viewed in:
  - Chrome DevTools: Performance panel > Load profile
  - Perfetto: https://ui.perfetto.dev/
"##
        }

        // === Record (video) ===
        "record" => {
            r##"
a3s use browser record - Record browser session to video

Usage: a3s use browser record start <path.webm> [url]
       a3s use browser record stop
       a3s use browser record restart <path.webm> [url]

Record the browser to a WebM video file.
Creates a fresh browser context but preserves cookies and localStorage.
If no URL is provided, automatically navigates to your current page.

Operations:
  start <path> [url]     Start recording (defaults to current URL if omitted)
  stop                   Stop recording and save video
  restart <path> [url]   Stop current recording (if any) and start a new one

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  # Record from current page (preserves login state)
  a3s use browser open https://app.example.com/dashboard
  a3s use browser snapshot -i            # Explore and plan
  a3s use browser record start ./demo.webm
  a3s use browser click @e3              # Execute planned actions
  a3s use browser record stop

  # Or specify a different URL
  a3s use browser record start ./demo.webm https://example.com

  # Restart recording with a new file (stops previous, starts new)
  a3s use browser record restart ./take2.webm
"##
        }

        // === Console/Errors ===
        "console" => {
            r##"
a3s use browser console - View console logs

Usage: a3s use browser console [--clear]

View browser console output (log, warn, error, info).

Options:
  --clear              Clear console log buffer

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser console
  a3s use browser console --clear
"##
        }
        "errors" => {
            r##"
a3s use browser errors - View page errors

Usage: a3s use browser errors [--clear]

View JavaScript errors and uncaught exceptions.

Options:
  --clear              Clear error buffer

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser errors
  a3s use browser errors --clear
"##
        }

        // === Highlight ===
        "highlight" => {
            r##"
a3s use browser highlight - Highlight an element

Usage: a3s use browser highlight <selector>

Visually highlights an element on the page for debugging.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser highlight "#target-element"
  a3s use browser highlight @e5
"##
        }

        // === Clipboard ===
        "clipboard" => {
            r##"
a3s use browser clipboard - Read and write clipboard

Usage: a3s use browser clipboard <operation> [text]

Read from or write to the browser clipboard.

Operations:
  read                 Read text from clipboard
  write <text>         Write text to clipboard
  copy                 Copy current selection (simulates Ctrl+C)
  paste                Paste from clipboard (simulates Ctrl+V)

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser clipboard read
  a3s use browser clipboard write "Hello, World!"
  a3s use browser clipboard copy
  a3s use browser clipboard paste
"##
        }

        // === State ===
        "state" => {
            r##"
a3s use browser state - Manage browser state

Usage: a3s use browser state <operation> [args]

Save, restore, list, and manage browser state (cookies, localStorage, sessionStorage).

Operations:
  save <path>                        Save current state to file
  load <path>                        Load state from file
  list                               List saved state files
  show <filename>                    Show state summary
  rename <old-name> <new-name>       Rename state file
  clear [session-name] [--all]       Clear saved states
  clean --older-than <days>          Delete expired state files

Automatic State Persistence:
  Use --restore to auto-save/restore state across restarts:
  a3s use browser --session myapp --restore open https://example.com
  Or set A3S_USE_BROWSER_RESTORE environment variable.

State Encryption:
  Set A3S_USE_BROWSER_ENCRYPTION_KEY (64-char hex) for AES-256-GCM encryption.
  Generate a key: openssl rand -hex 32

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser state save ./auth-state.json
  a3s use browser state load ./auth-state.json
  a3s use browser state list
  a3s use browser state show myapp-default.json
  a3s use browser state rename old-name new-name
  a3s use browser state clear --all
  a3s use browser state clean --older-than 7
"##
        }

        // === Session ===
        "session" => {
            r##"
a3s use browser session - Manage sessions

Usage: a3s use browser session [operation]

Manage isolated browser sessions. Each session has its own browser
instance with separate cookies, storage, and state.

Operations:
  (none)               Show current session name
  id                   Generate stable session id (--scope worktree|cwd|git-root, --prefix)
  info                 Show daemon, launch, and restore diagnostics
  list                 List all active sessions

Environment:
  A3S_USE_BROWSER_SESSION    Default session name
  A3S_USE_BROWSER_NAMESPACE  Namespace for daemon sockets and restore state

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session
  --namespace <name>   Use specific namespace

Examples:
  a3s use browser session
  a3s use browser session id --scope worktree --prefix next-dev-loop
  a3s use browser session info --json
  a3s use browser session list
  a3s use browser --session test open example.com
"##
        }

        // === Install ===
        "install" => {
            r##"
a3s use browser install - Install browser binaries

Usage: a3s use browser install [--with-deps]

Downloads and installs browser binaries required for automation.

Options:
  -d, --with-deps      Also install system dependencies (Linux only; fails if deps fail)

Examples:
  a3s use browser install
  a3s use browser install --with-deps
"##
        }

        // === Upgrade ===
        "upgrade" => {
            r##"
a3s use browser upgrade - Upgrade to the latest version

Usage: a3s use browser upgrade

Detects the current installation method (npm, Homebrew, or Cargo) and runs
the appropriate update command. Displays the version change on success, or
informs you if you are already on the latest version.

Examples:
  a3s use browser upgrade
"##
        }

        // === Doctor ===
        "doctor" => {
            r##"
a3s use browser doctor - Diagnose and repair your install

Usage: a3s use browser doctor [options]

Runs a battery of checks across environment, Chrome install, daemon state,
config files, encryption key, providers, network reachability, and a live
headless browser launch test.

Auto-cleans stale daemon socket/pid/version sidecar files. Destructive
repairs (reinstalling Chrome, purging old state files, generating a missing
encryption key) are gated behind --fix.

Options:
  --offline            Skip network probes
  --quick              Skip the live headless launch test
  --webgpu             Also run a live WebGPU render probe (renders via a real
                       WebGPU pass and pixel-checks both an in-page readback
                       and a decoded screenshot; launches a second Chrome)
  --headed             Run the WebGPU probe headed to validate the capture
                       path (auto-Xvfb on displayless Linux)
  --debug              Verbose diagnostics from the probes' scratch daemons
  --fix                Also run destructive repairs
  --json               JSON output

Exit codes:
  0  All checks pass (warnings OK)
  1  At least one check failed

Examples:
  a3s use browser doctor
  a3s use browser doctor --offline --quick
  a3s use browser doctor --webgpu
  a3s use browser doctor --webgpu --headed
  a3s use browser doctor --fix
  a3s use browser doctor --json
"##
        }

        // === Dashboard ===
        "dashboard" => {
            r##"
a3s use browser dashboard - Observability dashboard

Usage: a3s use browser dashboard [start|stop] [options]

Manage the observability dashboard, a local web UI that shows live
browser viewports and command activity feeds for all sessions.
The dashboard is bundled into the binary and requires no separate install.

Subcommands:
  start [--port <n>]   Start the dashboard server (default port: 4848)
  stop                 Stop the dashboard server

Running 'a3s use browser dashboard' with no subcommand is equivalent to 'dashboard start'.

The dashboard runs as a standalone background process, independent of
browser sessions. All sessions automatically stream to the dashboard.
It works from http://localhost:4848 or a proxied/forwarded URL that
reaches the dashboard server, such as https://dashboard.a3s-use-browser.localhost
or a Coder workspace URL. The browser stays on the dashboard origin;
session tabs, status, and stream traffic are proxied internally, so
session ports do not need to be exposed.

Options:
  --port <n>           Port for the dashboard server (default: 4848)

Global Options:
  --json               Output as JSON

Examples:
  a3s use browser dashboard start
  a3s use browser dashboard start --port 8080
  a3s use browser dashboard stop
"##
        }

        // === Connect ===
        "connect" => {
            r##"
a3s use browser connect - Connect to browser via CDP

Usage: a3s use browser connect <port|url>

Connects to a running browser instance via Chrome DevTools Protocol (CDP).
This allows controlling browsers, Electron apps, or remote browser services.

Arguments:
  <port>               Local port number (e.g., 9222)
  <url>                Full WebSocket URL (ws://, wss://, http://, https://)

Supported URL formats:
  - Port number: 9222 (connects to http://localhost:9222)
  - WebSocket URL: ws://localhost:9222/devtools/browser/...
  - Remote service: wss://remote-browser.example.com/cdp?token=...

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  # Connect to local Chrome with remote debugging
  # Start Chrome: google-chrome --remote-debugging-port=9222
  a3s use browser connect 9222

  # Connect using WebSocket URL from /json/version endpoint
  a3s use browser connect "ws://localhost:9222/devtools/browser/abc123"

  # Connect to remote browser service
  a3s use browser connect "wss://browser-service.example.com/cdp?token=xyz"

  # After connecting, run commands normally
  a3s use browser snapshot
  a3s use browser click @e1
"##
        }

        // === Runtime streaming ===
        "stream" => {
            r##"
a3s use browser stream - Manage live WebSocket browser streaming

Usage:
  a3s use browser stream enable [--port <port>]
  a3s use browser stream disable
  a3s use browser stream status

Enables or disables the session-scoped WebSocket stream server without restarting
an already-running daemon. If --port is omitted, a3s use browser binds an
available localhost port automatically and reports it back.

Notes:
  - 'stream enable' creates the WebSocket server.
  - WebSocket clients trigger frame streaming automatically.
  - 'screencast_start' and 'screencast_stop' still control explicit CDP screencasts.
  - Streaming is always enabled. Set A3S_USE_BROWSER_STREAM_PORT to bind to a
    specific port instead of the default OS-assigned port.

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser stream status
  a3s use browser stream enable
  a3s use browser stream enable --port 9223
  a3s use browser stream disable
"##
        }

        // === iOS Commands ===
        "tap" => {
            r##"
a3s use browser tap - Tap an element (touch gesture)

Usage: a3s use browser tap <selector>

Taps an element. This is an alias for 'click' that provides semantic clarity
for touch-based interfaces like iOS Safari.

Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser tap "#submit-button"
  a3s use browser tap @e1
  a3s use browser -p ios tap "button:has-text('Sign In')"
"##
        }
        "swipe" => {
            r##"
a3s use browser swipe - Swipe gesture (iOS)

Usage: a3s use browser swipe <direction> [distance]

Performs a swipe gesture on iOS Safari. The direction determines
which way the content moves (swipe up scrolls down, etc.).

Arguments:
  direction    up, down, left, or right
  distance     Optional distance in pixels (default: 300)

Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser -p ios swipe up
  a3s use browser -p ios swipe down 500
  a3s use browser -p ios swipe left
"##
        }
        "device" => {
            r##"
a3s use browser device - Manage iOS simulators

Usage: a3s use browser device <subcommand>

Subcommands:
  list    List available iOS simulators

Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser device list
  a3s use browser -p ios device list
"##
        }

        "diff" => {
            r##"
a3s use browser diff - Compare page states

Subcommands:

  diff snapshot                   Compare current snapshot to last snapshot in session
  diff screenshot --baseline <f>  Visual pixel diff against a baseline image
  diff url <url1> <url2>          Compare two pages

Snapshot Diff:

  Usage: a3s use browser diff snapshot [options]

  Options:
    -b, --baseline <file>    Compare against a saved snapshot file
    -s, --selector <sel>     Scope snapshot to a CSS selector or @ref
    -c, --compact            Use compact snapshot format
    -d, --depth <n>          Limit snapshot tree depth

  Without --baseline, compares against the last snapshot taken in this session.

Screenshot Diff:

  Usage: a3s use browser diff screenshot --baseline <file> [options]

  Options:
    -b, --baseline <file>    Baseline image to compare against (required)
    -o, --output <file>      Path for the diff image (default: temp dir)
    -t, --threshold <0-1>    Color distance threshold (default: 0.1)
    -s, --selector <sel>     Scope screenshot to element
        --full               Full page screenshot

URL Diff:

  Usage: a3s use browser diff url <url1> <url2> [options]

  Options:
    --screenshot             Also compare screenshots (default: snapshot only)
    --full                   Full page screenshots
    --wait-until <strategy>  Navigation wait strategy: load, domcontentloaded, networkidle (default: load)
    -s, --selector <sel>     Scope snapshots to a CSS selector or @ref
    -c, --compact            Use compact snapshot format
    -d, --depth <n>          Limit snapshot tree depth

Global Options:
  --json               Output as JSON
  --session <name>     Use specific session

Examples:
  a3s use browser diff snapshot
  a3s use browser diff snapshot --baseline before.txt
  a3s use browser diff screenshot --baseline before.png
  a3s use browser diff screenshot --baseline before.png --output diff.png --threshold 0.2
  a3s use browser diff url https://staging.example.com https://prod.example.com
  a3s use browser diff url https://v1.example.com https://v2.example.com --screenshot
"##
        }

        "batch" => {
            r##"
a3s use browser batch - Execute multiple commands sequentially

Usage: a3s use browser batch [options] "<cmd1>" "<cmd2>" ...
       echo '<json>' | a3s use browser batch [options]

Runs multiple commands in sequence. Commands can be passed as quoted
arguments or piped as JSON via stdin. Results are printed in order,
separated by blank lines (or as a JSON array with --json).

Options:
  --bail               Stop on first error (default: continue all commands)
  --json               Output results as a JSON array

Argument Mode:
  Each quoted argument is a full command string:
  a3s use browser batch "open https://example.com" "snapshot -i" "screenshot"

Stdin Mode (JSON):
  A JSON array of string arrays. Each inner array is one command:
  [
    ["open", "https://example.com"],
    ["snapshot", "-i"],
    ["click", "@e1"],
    ["fill", "@e2", "test@example.com"],
    ["screenshot", "result.png"]
  ]

Examples:
  a3s use browser batch "open https://example.com" "screenshot"
  a3s use browser batch --bail "open https://example.com" "click @e1" "screenshot"
  echo '[["open", "https://example.com"], ["snapshot"]]' | a3s use browser batch
  a3s use browser batch --bail < commands.json
"##
        }

        "profiles" => {
            r##"
a3s use browser profiles - List available Chrome profiles

Usage: a3s use browser profiles

Lists all Chrome profiles found in your Chrome user data directory, showing
the directory name and display name for each profile. Use the directory name
with --profile to launch Chrome with that profile's login state.

Global Options:
  --json               Output as JSON

Examples:
  a3s use browser profiles
  a3s use browser profiles --json
  a3s use browser --profile Default open https://gmail.com
"##
        }

        "chat" => {
            r##"
a3s use browser chat - Natural language browser control via AI

Usage:
  a3s use browser chat <message>         Single-shot: execute instruction and exit
  a3s use browser chat                   Interactive REPL (when stdin is a TTY)
  echo "instruction" | a3s use browser chat   Piped input

Sends natural language instructions to an AI model that translates them
into a3s use browser commands and executes them against the active session.
Requires AI_GATEWAY_API_KEY to be set.

In interactive mode, type "quit", "exit", or "q" to leave the REPL.

Chat Options:
  --model <name>         AI model (or AI_GATEWAY_MODEL env, default: anthropic/claude-sonnet-4.6)
  -v, --verbose          Show tool commands and their raw output
  -q, --quiet            Show only the AI text response (hide tool calls)

Global Options:
  --json                 Structured JSON output per turn
  --session <name>       Target session for commands

Examples:
  a3s use browser chat "open google.com and search for cats"
  a3s use browser chat "take a screenshot of the current page"
  a3s use browser -q chat "summarize this page"
  a3s use browser -v chat "fill in the login form with test@example.com"
  a3s use browser --model openai/gpt-4o chat "navigate to hacker news"
  a3s use browser chat
"##
        }

        "mcp" => {
            r##"
a3s use browser mcp - Start an MCP stdio server

Usage: a3s use browser mcp [--tools <profiles>]

Starts a Model Context Protocol server over stdio. MCP clients launch this
command as a subprocess and communicate with newline-delimited JSON-RPC.
stdout is reserved for MCP protocol messages; logs and diagnostics use stderr.
The server defaults to MCP protocol 2025-11-25 and accepts older supported
client protocol versions during initialization.

The default tools profile is core, which keeps MCP context small for everyday
browser automation. Use --tools all for the full typed CLI parity surface, or
combine profiles with commas, such as --tools core,network,react.

Tool profiles:
  core       Default. Navigation, snapshots, interaction, waits, reads,
             screenshots, JavaScript eval, close, tab basics, and profile discovery
  network    Network routes, request inspection, HAR, headers, credentials, offline
  state      Cookies, storage, auth, saved state, sessions, profiles, skills
  debug      Console/errors, tracing, profiling, recording, clipboard, plugins,
             doctor, dashboard, install, upgrade, chat, diff, batch, confirm/deny
  tabs       Back/forward/reload, tabs, windows, frames, dialogs
  react      React tree/inspect/renders/suspense, vitals, pushstate
  mobile     Viewport/device/geolocation/media, touch, swipe, mouse, keyboard
  all        Every MCP tool, including the full typed CLI parity surface

Common tools include:
  agent_browser_tools_profiles  List MCP startup tool profiles
  agent_browser_open       Open a URL or launch about:blank
  agent_browser_snapshot   Get an accessibility snapshot with refs
  agent_browser_click      Click an element by @ref or selector
  agent_browser_fill       Fill an input
  agent_browser_screenshot Take a screenshot
  agent_browser_get_url    Read the current URL
  agent_browser_close      Close the browser session

Each tool has typed fields such as url, selector, text, key, and session.
Each tool also accepts extraArgs for advanced CLI flags and exact CLI parity.
Tool discovery is paginated and includes read-only/open-world annotations so
modern MCP clients can load the large typed surface incrementally.
Use agent_browser_snapshot after navigation to get fresh refs before clicking.

MCP client config example:
  {
    "mcpServers": {
      "a3s use browser": {
        "command": "a3s use browser",
        "args": ["mcp"]
      }
    }
  }

Full parity config example:
  {
    "mcpServers": {
      "a3s use browser": {
        "command": "a3s use browser",
        "args": ["mcp", "--tools", "all"]
      }
    }
  }

Environment:
  A3S_USE_BROWSER_SESSION          Default browser session
  A3S_USE_BROWSER_SOCKET_DIR       Daemon socket directory
  A3S_USE_BROWSER_CONFIG           Config file loaded by tool invocations
"##
        }

        "skills" => {
            r##"
a3s use browser skills - List and retrieve bundled skill content

Usage: a3s use browser skills [subcommand] [options]

Subcommands:
  list                       List all available skills (default)
  get <name> [name...]       Output a skill's full content
  get <name> --full          Include references and templates
  get --all                  Output every skill
  path [name]                Print filesystem path to skill directory

Options:
  --json                     Output as JSON

The skills command serves bundled skill content that always matches the
installed CLI version. Agents should use this to get current instructions
rather than relying on cached copies.

Examples:
  a3s use browser skills
  a3s use browser skills list
  a3s use browser skills get core
  a3s use browser skills get core --full
  a3s use browser skills get electron --full
  a3s use browser skills get --all
  a3s use browser skills path core
  a3s use browser skills list --json

Environment:
  A3S_USE_BROWSER_SKILLS_DIR   Override the skills directory path
"##
        }

        "plugin" | "plugins" => {
            r##"
a3s use browser plugin - Manage configured plugins

Usage: a3s use browser plugin [subcommand]

Subcommands:
  add <ref>                Add a plugin from npm or GitHub
  list                     List configured plugins (default)
  show <name>              Show one configured plugin
  run <name> <type>        Run a command.run or custom plugin request

Plugins are configured in A3S ACL. A plugin block declares a name,
an executable command, optional args, and capabilities. Plugins run as
external processes over the agent-browser.plugin.v1 compatibility protocol.

Add sources:
  <name>                   npm package, e.g. agent-browser-plugin-captcha
  @<scope>/<name>          scoped npm package
  <owner>/<repo>           GitHub repository

Add options:
  --name <name>            Override the configured plugin name
  --capability <name>      Declare a capability if the plugin has no manifest
  --global                 Write ~/.a3s/use/browser/config.acl instead of ./.a3s/use/browser.acl
  --no-manifest            Skip plugin.manifest discovery

plugin add asks the package for plugin.manifest to discover name and
capabilities. Use --capability when adding older plugins without a manifest.

Capabilities:
  credential.read          Resolve credentials for auth login
  browser.provider         Launch/connect an external browser provider
  launch.mutate            Append local launch args, extensions, or init scripts
  command.run              Accept arbitrary namespaced plugin requests

Core capabilities and protocol request types use dedicated command paths.
Use auth login for credential.read, --provider for browser.provider, and
a local launch for launch.mutate.

Example ACL config:
  browser {{
    plugin "vault" {{
      command = "agent-browser-plugin-vault"
      args = []
      capabilities = ["credential.read"]
    }}
  }}

Examples:
  a3s use browser plugin add agent-browser-plugin-captcha
  a3s use browser plugin add org/agent-browser-plugin-cloud-browser
  a3s use browser plugin add @company/agent-browser-plugin-vault --name vault
  a3s use browser plugin list
  a3s use browser plugin show vault
  a3s use browser plugin run captcha captcha.solve --payload '{{"siteKey":"...","url":"https://example.com"}}'
  a3s use browser auth login my-app --credential-provider vault --item "My App"
  a3s use browser --provider cloud-browser open https://example.com
"##
        }

        _ => return false,
    };
    println!("{}", help.trim());
    true
}

pub fn print_help() {
    println!(
        r#"
a3s use browser - fast browser automation CLI for AI agents

Usage: a3s use browser <command> [args] [options]

Start here (for AI agents):
  a3s use browser skills get core --full

  Skills ship with the CLI (always version-matched) and include workflow
  patterns, ref/selector usage, and copy-paste examples. Prefer this over
  guessing commands from flag docs alone. Specialized skills cover Electron
  apps, Slack, exploratory testing, and cloud browser providers.

  skills [list]                List available skills
  skills get core              Core usage guide (overview + common patterns)
  skills get core --full       Include full command reference and templates
  skills get <name>            Load a specialized skill (electron, slack, ...)
  skills path [name]           Print skill directory path

Core Commands:
  open <url>                 Navigate to URL
  read [url]                 Fetch agent-readable text
  click <sel>                Click element (or @ref)
  dblclick <sel>             Double-click element
  type <sel> <text>          Type into element
  fill <sel> <text>          Clear and fill
  press <key>                Press key (Enter, Tab, Control+a)
  keyboard type <text>       Type text with real keystrokes (no selector)
  keyboard inserttext <text> Insert text without key events
  hover <sel>                Hover element
  focus <sel>                Focus element
  check <sel>                Check checkbox
  uncheck <sel>              Uncheck checkbox
  select <sel> <val...>      Select dropdown option
  drag <src> <dst>           Drag and drop
  upload <sel> <files...>    Upload files
  download <sel> <path>      Download file by clicking element
  scroll <dir> [px]          Scroll (up/down/left/right)
  scrollintoview <sel>       Scroll element into view
  wait <sel|ms>              Wait for element or time
  screenshot [path]          Take screenshot
  pdf <path>                 Save as PDF
  snapshot                   Accessibility tree with refs (for AI)
  eval <js>                  Run JavaScript
  connect <port|url>         Connect to browser via CDP
  close [--all]              Close browser (--all closes every session)

Navigation:
  back                       Go back
  forward                    Go forward
  reload                     Reload page

Get Info:  a3s use browser get <what> [selector]
  text, html, value, attr <name>, title, url, count, box, styles, cdp-url

Check State:  a3s use browser is <what> <selector>
  visible, enabled, checked

Find Elements:  a3s use browser find <locator> <value> <action> [text]
  role, text, label, placeholder, alt, title, testid, first, last, nth

Mouse:  a3s use browser mouse <action> [args]
  move <x> <y>, down [btn], up [btn], wheel <dy> [dx]

Browser Settings:  a3s use browser set <setting> [value]
  viewport <w> <h>, device <name>, geo <lat> <lng>
  offline [on|off], headers <json>, credentials <user> <pass>
  media [dark|light] [reduced-motion]

Network:  a3s use browser network <action>
  route <url> [--abort|--body <json>] [--resource-type <csv>]
  unroute [url]
  requests [--clear] [--filter <pattern>]
  har <start|stop> [path]

Storage:
  cookies [get|set|clear]    Manage cookies (set supports --url, --domain, --path, --httpOnly, --secure, --sameSite, --expires)
                             Or:  cookies set --curl <file> [--domain <host>] (auto-detects JSON/cURL/Cookie-header files)
  storage <local|session>    Manage web storage

Tabs:
  tab [new|list|close|<n>]   Manage tabs

Diff:
  diff snapshot              Compare current vs last snapshot
  diff screenshot --baseline Compare current vs baseline image
  diff url <u1> <u2>         Compare two pages

Debug:
  trace start                Start Chrome DevTools trace
  trace stop [path]          Stop and save Chrome DevTools trace
  profiler start|stop [path] Record Chrome DevTools profile
  record start <path> [url]  Start video recording (WebM)
  record stop                Stop and save video
  console [--clear]          View console logs
  errors [--clear]           View page errors
  highlight <sel>            Highlight element
  inspect                    Open Chrome DevTools for the active page
  clipboard <op> [text]      Read/write clipboard (read, write, copy, paste)

Streaming:
  stream enable [--port <n>] Start runtime WebSocket streaming for this session
  stream disable             Stop runtime WebSocket streaming
  stream status              Show streaming status and active port

React (requires `open --enable react-devtools`):
  react tree                 Full React component tree (depth id parent name columns)
  react inspect <id>         Inspect one fiber (props, hooks, state, source)
  react renders start        Start recording re-renders via onCommitFiberRoot
  react renders stop [--json] Stop and print render profile
  react suspense [--only-dynamic] [--json]
                             Walk Suspense boundaries + classifier report
                             --only-dynamic hides the "static" list

Performance:
  vitals [url] [--json]      Core Web Vitals (LCP/CLS/TTFB/FCP/INP) +
                             React hydration summary; --json returns full data

SPA:
  pushstate <url>            SPA client-side nav. Auto-detects window.next.router.push
                             (triggers RSC fetch on Next.js); falls back to
                             history.pushState + popstate/navigate events for other frameworks

Init scripts:
  removeinitscript <id>      Remove a script registered via --init-script or addinitscript

Batch:
  batch [--bail] ["cmd" ...]  Execute multiple commands sequentially (args or stdin)
                              --bail stops on first error (default: continue all)

Auth Vault:
  auth save <name> [opts]    Save auth profile (--url, --username, --password/--password-stdin)
  auth login <name>          Login using saved credentials (waits for form fields)
  auth login <name> --credential-provider <plugin> [--item <ref>] [--url <url>]
                             Resolve credentials from a configured plugin
  auth login <name> --username-selector <s> --password-selector <s>
                             Override selectors for one login
  auth list                  List saved auth profiles
  auth show <name>           Show auth profile metadata
  auth delete <name>         Delete auth profile

Plugins:
  plugin add <ref>           Add a plugin from npm or GitHub
  plugin [list]              List configured plugins
  plugin show <name>         Show one configured plugin
  plugin run <name> <type>   Run a command.run or custom plugin request

Confirmation:
  confirm <id>               Approve a pending action
  deny <id>                  Deny a pending action

Sessions:
  session                    Show current session name
  session list               List active sessions

MCP:
  mcp                        Start an MCP stdio server exposing a3s use browser tools

Chat (AI):
  chat <message>             Send a natural language instruction (single-shot)
  chat                       Start interactive chat (REPL mode when stdin is a TTY)
  Options: --model <name>, -v/--verbose, -q/--quiet

Dashboard:
  dashboard [start]          Start the dashboard server (default port: 4848)
  dashboard start --port <n> Start on a specific port
  dashboard stop             Stop the dashboard server

Setup:
  install                    Install browser binaries
  install --with-deps        Also install system dependencies (Linux)
  upgrade                    Upgrade to the latest version
  doctor [--fix]             Diagnose install; auto-clean stale files
  dashboard start            Start the observability dashboard
  profiles                   List available Chrome profiles

Snapshot Options:
  -i, --interactive          Only interactive elements
  -c, --compact              Remove empty structural elements
  -d, --depth <n>            Limit tree depth
  -s, --selector <sel>       Scope to CSS selector

Authentication:
  --profile <name|path>      Chrome profile name (e.g., Default) to reuse login state,
                             or a directory path for a persistent custom profile
                             (or A3S_USE_BROWSER_PROFILE env)
  --restore [name]           Auto-save/restore cookies and localStorage.
                             Without a name, uses --session as the restore key
                             (or A3S_USE_BROWSER_RESTORE env)
  --restore-save <policy>    Restore auto-save policy: auto, always, never (default: auto)
  --restore-check-url <glob> Validate restored state against current URL pattern
  --restore-check-text <txt> Validate restored state against visible page text
  --restore-check-fn <js>    Validate restored state against a truthy JS expression
  --session-name <name>      Legacy alias for restore persistence key
                             (or A3S_USE_BROWSER_SESSION_NAME env)
  --state <path>             Load saved auth state (cookies + storage) from JSON file
                             (or A3S_USE_BROWSER_STATE env)
  --auto-connect             Connect to a running Chrome to reuse its auth state
                             Tip: a3s use browser --auto-connect state save ./auth.json
  --headers <json>           HTTP headers scoped to URL's origin (e.g., Authorization bearer token)

Options:
  --session <name>           Isolated session (or A3S_USE_BROWSER_SESSION env)
  --namespace <name>         Isolate daemon sockets and restore-state directories
                             (or A3S_USE_BROWSER_NAMESPACE env)
  --executable-path <path>   Custom browser executable (or A3S_USE_BROWSER_EXECUTABLE_PATH)
  --extension <path>         Load browser extensions (repeatable)
  --init-script <path>       Register a page init script before the first navigation (repeatable)
                             (or A3S_USE_BROWSER_INIT_SCRIPTS env, comma-separated)
  --enable <feature>         Built-in init scripts: react-devtools (repeatable or comma-separated)
                             (or A3S_USE_BROWSER_ENABLE env)
  --args <args>              Browser launch args, comma or newline separated (or A3S_USE_BROWSER_ARGS)
                             e.g., --args "--no-sandbox,--disable-blink-features=AutomationControlled"
  --user-agent <ua>          Custom User-Agent (or A3S_USE_BROWSER_USER_AGENT)
  --proxy <server>           Proxy server URL (or A3S_USE_BROWSER_PROXY, HTTP_PROXY, HTTPS_PROXY, ALL_PROXY)
                             Supports authenticated proxies: --proxy "http://user:pass@127.0.0.1:7890"
  --proxy-bypass <hosts>     Bypass proxy for these hosts (or A3S_USE_BROWSER_PROXY_BYPASS, NO_PROXY)
                             e.g., --proxy-bypass "localhost,*.internal.com"
  --ignore-https-errors      Ignore HTTPS certificate errors
  --allow-file-access        Allow file:// URLs to access local files (Chromium only)
  --hide-scrollbars <bool>   Hide native scrollbars in headless Chromium screenshots (default: true)
                             Use --hide-scrollbars false to keep scrollbars visible
  -p, --provider <name>      Browser provider: ios, browserbase, kernel, browseruse, browserless, agentcore, or plugin name
  --device <name>            iOS device name (e.g., "iPhone 15 Pro")
  --json                     JSON output
  --annotate                 Annotated screenshot with numbered labels and legend
  --screenshot-dir <path>    Default screenshot output directory (or A3S_USE_BROWSER_SCREENSHOT_DIR)
  --screenshot-quality <n>   JPEG quality 0-100; ignored for PNG (or A3S_USE_BROWSER_SCREENSHOT_QUALITY)
  --screenshot-format <fmt>  Screenshot format: png, jpeg (or A3S_USE_BROWSER_SCREENSHOT_FORMAT)
  --headed                   Show browser window (not headless) (or A3S_USE_BROWSER_HEADED env)
  --webgpu                   Enable WebGPU; uses SwiftShader software Vulkan on Linux, no GPU required (or A3S_USE_BROWSER_WEBGPU env)
  --cdp <port>               Connect via CDP (Chrome DevTools Protocol)
  --color-scheme <scheme>    Color scheme: dark, light, no-preference (or A3S_USE_BROWSER_COLOR_SCHEME)
  --download-path <path>     Default download directory (or A3S_USE_BROWSER_DOWNLOAD_PATH)
  --content-boundaries       Wrap page output in boundary markers (or A3S_USE_BROWSER_CONTENT_BOUNDARIES)
  --max-output <chars>       Truncate page output to N chars (or A3S_USE_BROWSER_MAX_OUTPUT)
  --allowed-domains <list>   Restrict navigation domains (or A3S_USE_BROWSER_ALLOWED_DOMAINS)
  --action-policy <path>     Action policy JSON file (or A3S_USE_BROWSER_ACTION_POLICY)
  --confirm-actions <list>   Categories requiring confirmation (or A3S_USE_BROWSER_CONFIRM_ACTIONS)
  --confirm-interactive      Interactive confirmation prompts; auto-denies if stdin is not a TTY (or A3S_USE_BROWSER_CONFIRM_INTERACTIVE)
  --engine <name>            Browser engine: chrome (default), lightpanda (or A3S_USE_BROWSER_ENGINE)
  --no-auto-dialog           Disable automatic dismissal of alert/beforeunload dialogs (or A3S_USE_BROWSER_NO_AUTO_DIALOG)
  --model <name>             AI model for chat (or AI_GATEWAY_MODEL env)
  -v, --verbose              Show tool commands and their raw output
  -q, --quiet                Show only AI text responses (hide tool calls)
  --config <path>            Use a custom config file (or A3S_USE_BROWSER_CONFIG env)
  --debug                    Debug output
  --version, -V              Show version

Configuration:
  a3s use browser looks for A3S ACL in these locations (lowest to highest priority):
    1. ~/.a3s/use/browser/config.acl       User-level defaults
    2. ./.a3s/use/browser.acl              Project-level overrides
    3. Environment variables             Override config file values
    4. CLI flags                         Override everything

  Use --config <path> to load a specific config file instead of the defaults.
  If --config points to a missing or invalid file, a3s use browser exits with an error.

  Boolean flags accept an optional true/false value to override config:
    --headed           (same as --headed true)
    --headed false     (disables "headed": true from config)
    --hide-scrollbars false (keeps native scrollbars visible in headless Chromium screenshots)

  Extensions from user and project configs are merged (not replaced).

  Example Browser ACL:
    browser {{
      headed = true
      hide_scrollbars = false
      proxy = "http://localhost:8080"
    }}

  Plugin example:
    browser {{
      plugin "vault" {{
        command = "agent-browser-plugin-vault"
        args = []
        capabilities = ["credential.read"]
      }}
    }}

Environment:
  A3S_USE_BROWSER_CONFIG           Path to config file (or use --config)
  A3S_USE_BROWSER_SESSION          Session name (default: "default")
  A3S_USE_BROWSER_NAMESPACE        Namespace for daemon sockets and restore state
  A3S_USE_BROWSER_RESTORE          Auto-save/restore persistence key
  A3S_USE_BROWSER_RESTORE_SAVE     Restore save policy: auto, always, never
  A3S_USE_BROWSER_AUTOSAVE_INTERVAL_MS Min ms between periodic session autosaves (default: 30000, 0 disables)
  A3S_USE_BROWSER_RESTORE_CHECK_URL URL pattern restored state must match
  A3S_USE_BROWSER_RESTORE_CHECK_TEXT Page text restored state must contain
  A3S_USE_BROWSER_RESTORE_CHECK_FN JS expression restored state must satisfy
  A3S_USE_BROWSER_SESSION_NAME     Legacy auto-save/restore state persistence name
  A3S_USE_BROWSER_ENCRYPTION_KEY   64-char hex key for AES-256-GCM state encryption
  A3S_USE_BROWSER_STATE_EXPIRE_DAYS Auto-delete states older than N days (default: 30)
  A3S_USE_BROWSER_EXECUTABLE_PATH  Custom browser executable path
  A3S_USE_BROWSER_EXTENSIONS       Comma-separated browser extension paths
  A3S_USE_BROWSER_INIT_SCRIPTS     Comma-separated paths to page init scripts
  A3S_USE_BROWSER_ENABLE           Comma-separated built-in init script features (e.g. react-devtools)
  A3S_USE_BROWSER_HEADED           Show browser window (not headless)
  A3S_USE_BROWSER_NO_XVFB          Disable automatic Xvfb for headed mode on displayless Linux hosts
  A3S_USE_BROWSER_WEBGPU           Enable WebGPU (SwiftShader software Vulkan on Linux)
  A3S_USE_BROWSER_JSON             JSON output
  A3S_USE_BROWSER_ANNOTATE         Annotated screenshot with numbered labels and legend
  A3S_USE_BROWSER_DEBUG            Debug output
  A3S_USE_BROWSER_IGNORE_HTTPS_ERRORS Ignore HTTPS certificate errors
  A3S_USE_BROWSER_PROVIDER         Browser provider (ios, browserbase, kernel, browseruse, browserless, agentcore, or plugin name)
  A3S_USE_BROWSER_AUTO_CONNECT     Auto-discover and connect to running Chrome
  A3S_USE_BROWSER_ALLOW_FILE_ACCESS Allow file:// URLs to access local files
  A3S_USE_BROWSER_HIDE_SCROLLBARS  Hide scrollbars in headless Chromium screenshots (default: true)
  A3S_USE_BROWSER_COLOR_SCHEME     Color scheme preference (dark, light, no-preference)
  A3S_USE_BROWSER_DOWNLOAD_PATH    Default download directory for browser downloads
  A3S_USE_BROWSER_DEFAULT_TIMEOUT  Default action timeout in ms (default: 25000)
  A3S_USE_BROWSER_SESSION_NAME     Legacy auto-save/load state persistence name
  A3S_USE_BROWSER_STATE_EXPIRE_DAYS Auto-delete saved states older than N days (default: 30)
  A3S_USE_BROWSER_ENCRYPTION_KEY   64-char hex key for AES-256-GCM session encryption
  A3S_USE_BROWSER_STREAM_PORT      Override WebSocket streaming port (default: OS-assigned)
  A3S_USE_BROWSER_IDLE_TIMEOUT_MS  Auto-shutdown daemon after N ms of inactivity (disabled by default)
  A3S_USE_BROWSER_IOS_DEVICE       Default iOS device name
  A3S_USE_BROWSER_IOS_UDID         Default iOS device UDID
  A3S_USE_BROWSER_CONTENT_BOUNDARIES Wrap page output in boundary markers
  A3S_USE_BROWSER_MAX_OUTPUT       Max characters for page output
  A3S_USE_BROWSER_ALLOWED_DOMAINS  Comma-separated allowed domain patterns
  A3S_USE_BROWSER_ACTION_POLICY    Path to action policy JSON file
  A3S_USE_BROWSER_CONFIRM_ACTIONS  Action categories requiring confirmation
  A3S_USE_BROWSER_CONFIRM_INTERACTIVE Enable interactive confirmation prompts
  A3S_USE_BROWSER_NO_AUTO_DIALOG   Disable automatic dismissal of alert/beforeunload dialogs
  A3S_USE_BROWSER_ENGINE           Browser engine: chrome (default), lightpanda
  A3S_USE_BROWSER_PLUGINS          JSON plugin registry override
  HTTP_PROXY / HTTPS_PROXY       Standard proxy env vars (fallback if A3S_USE_BROWSER_PROXY not set)
  ALL_PROXY                      SOCKS proxy (fallback for proxy)
  NO_PROXY                       Bypass proxy for hosts (fallback for proxy-bypass)
  A3S_USE_BROWSER_SCREENSHOT_DIR   Default screenshot output directory
  A3S_USE_BROWSER_SCREENSHOT_QUALITY JPEG quality 0-100
  A3S_USE_BROWSER_SCREENSHOT_FORMAT Screenshot format: png, jpeg
  AI_GATEWAY_URL                 Vercel AI Gateway base URL (default: https://ai-gateway.vercel.sh)
  AI_GATEWAY_API_KEY             API key for the AI Gateway (enables chat command and dashboard AI chat)
  AI_GATEWAY_MODEL               Default AI model (default: anthropic/claude-sonnet-4.6, or --model flag)

Install and upgrade:
  a3s install use                          # Install or repair A3S Use
  a3s install use/browser                  # Install the managed Browser runtime
  a3s upgrade use                          # Upgrade through the trusted component catalog
  a3s use browser install                  # Compatibility alias for Browser runtime install

Examples:
  a3s use browser open example.com
  a3s use browser snapshot -i              # Interactive elements only
  a3s use browser click @e2                # Click by ref from snapshot
  a3s use browser fill @e3 "test@example.com"
  a3s use browser find role button click --name Submit
  a3s use browser get text @e1
  a3s use browser screenshot --full
  a3s use browser screenshot --annotate    # Labeled screenshot for vision models
  a3s use browser wait 2000               # Wait for slow pages to settle
  a3s use browser --cdp 9222 snapshot      # Connect via CDP port
  a3s use browser --auto-connect snapshot  # Auto-discover running Chrome
  a3s use browser stream enable            # Start runtime streaming on an auto-selected port
  a3s use browser stream status            # Inspect runtime streaming state
  a3s use browser --color-scheme dark open example.com  # Dark mode
  a3s use browser --profile Default open gmail.com        # Reuse Chrome login state
  a3s use browser --profile ~/.myapp open example.com    # Persistent custom profile
  a3s use browser profiles                               # List available Chrome profiles
  SESSION="$(a3s use browser session id --scope worktree --prefix myapp)"
  a3s use browser --session "$SESSION" --restore open example.com  # Auto-save/restore state
  a3s use browser session info --json                    # Inspect daemon and restore status
  a3s use browser chat "open google.com and search for cats"  # AI chat (single-shot)
  a3s use browser chat                                        # AI chat (interactive REPL)
  a3s use browser -q chat "summarize this page"               # Quiet mode (text only)

Command Chaining:
  Chain commands with && in a single shell call (browser persists via daemon):

  a3s use browser open example.com && a3s use browser snapshot -i
  a3s use browser fill @e1 "user@example.com" && a3s use browser fill @e2 "pass" && a3s use browser click @e3
  a3s use browser open example.com && a3s use browser screenshot

iOS Simulator (requires Xcode and Appium):
  a3s use browser -p ios open example.com                    # Use default iPhone
  a3s use browser -p ios --device "iPhone 15 Pro" open url   # Specific device
  a3s use browser -p ios device list                         # List simulators
  a3s use browser -p ios swipe up                            # Swipe gesture
  a3s use browser -p ios tap @e1                             # Touch element
"#
    );
}

fn print_snapshot_diff(data: &serde_json::Map<String, serde_json::Value>) {
    let changed = data
        .get("changed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !changed {
        println!("{} No changes detected", color::success_indicator());
        return;
    }
    if let Some(diff) = data.get("diff").and_then(|v| v.as_str()) {
        for line in diff.lines() {
            if line.starts_with("+ ") {
                println!("{}", color::green(line));
            } else if line.starts_with("- ") {
                println!("{}", color::red(line));
            } else {
                println!("{}", color::dim(line));
            }
        }
        let additions = data.get("additions").and_then(|v| v.as_i64()).unwrap_or(0);
        let removals = data.get("removals").and_then(|v| v.as_i64()).unwrap_or(0);
        let unchanged = data.get("unchanged").and_then(|v| v.as_i64()).unwrap_or(0);
        println!(
            "\n{} additions, {} removals, {} unchanged",
            color::green(&additions.to_string()),
            color::red(&removals.to_string()),
            unchanged
        );
    }
}

fn print_screenshot_diff(data: &serde_json::Map<String, serde_json::Value>) {
    let mismatch = data
        .get("mismatchPercentage")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let is_match = data.get("match").and_then(|v| v.as_bool()).unwrap_or(false);
    let dim_mismatch = data
        .get("dimensionMismatch")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if dim_mismatch {
        println!(
            "{} Images have different dimensions",
            color::error_indicator()
        );
    } else if is_match {
        println!(
            "{} Images match (0% difference)",
            color::success_indicator()
        );
    } else {
        println!(
            "{} {:.2}% pixels differ",
            color::error_indicator(),
            mismatch
        );
    }
    if let Some(diff_path) = data.get("diffPath").and_then(|v| v.as_str()) {
        println!("  Diff image: {}", color::green(diff_path));
    }
    let total = data
        .get("totalPixels")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let different = data
        .get("differentPixels")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    println!(
        "  {} different / {} total pixels",
        color::red(&different.to_string()),
        total
    );
}

pub fn print_version() {
    println!("a3s use browser {}", env!("CARGO_PKG_VERSION"));
}

#[cfg(test)]
mod tests {
    use super::{
        boundary_origin, format_storage_text, format_vitals_text, format_with_boundaries,
        OutputOptions,
    };
    use serde_json::json;

    #[test]
    fn test_format_stream_status_text_for_enabled_stream() {
        let data = json!({
            "enabled": true,
            "port": 9223,
            "connected": true,
            "screencasting": false
        });

        let rendered = super::format_stream_status_text(Some("stream_status"), &data).unwrap();

        assert_eq!(
            rendered,
            "Streaming enabled on ws://127.0.0.1:9223\nConnected: true\nScreencasting: false"
        );
    }

    #[test]
    fn test_format_stream_status_text_for_disabled_stream() {
        let data =
            json!({ "enabled": false, "port": null, "connected": false, "screencasting": false });

        let rendered = super::format_stream_status_text(Some("stream_status"), &data).unwrap();

        assert_eq!(rendered, "Streaming disabled");
    }

    #[test]
    fn test_format_storage_text_for_all_entries() {
        let data = json!({
            "data": {
                "token": "abc123",
                "user": "alice"
            }
        });

        let rendered = format_storage_text(&data).unwrap();

        assert_eq!(rendered, "token: abc123\nuser: alice");
    }

    #[test]
    fn test_format_storage_text_for_key_lookup() {
        let data = json!({
            "key": "token",
            "value": "abc123"
        });

        let rendered = format_storage_text(&data).unwrap();

        assert_eq!(rendered, "token: abc123");
    }

    #[test]
    fn test_format_storage_text_for_empty_store() {
        let data = json!({
            "data": {}
        });

        let rendered = format_storage_text(&data).unwrap();

        assert_eq!(rendered, "No storage entries");
    }

    #[test]
    fn test_format_vitals_text_summary() {
        let data = json!({
            "url": "https://example.com/dashboard",
            "ttfb": 12.34,
            "fcp": 56.0,
            "lcp": {
                "startTime": 123.45,
                "size": 1200,
                "element": "img",
                "url": "https://example.com/assets/hero.png"
            },
            "cls": {
                "score": 0.0123,
                "entries": []
            },
            "inp": null,
            "hydration": {
                "startTime": 130.0,
                "endTime": 180.25,
                "duration": 50.25
            },
            "phases": [{ "label": "Hydrated" }],
            "hydratedComponents": [{ "name": "App" }, { "name": "Nav" }]
        });

        let rendered = format_vitals_text(&data);

        assert_eq!(
            rendered,
            "url: https://example.com/dashboard\n\
ttfb: 12.34ms  fcp: 56ms  lcp: 123.45ms  cls: 0.01  inp: -\n\
lcp: element: img  asset: https://example.com/assets/hero.png\n\
hydration: 50.25ms  phases: 1  hydratedComponents: 2"
        );
    }

    #[test]
    fn test_format_vitals_text_handles_missing_values() {
        let data = json!({
            "url": "https://example.com",
            "lcp": null,
            "cls": { "score": 0.0, "entries": [] },
            "phases": [],
            "hydratedComponents": []
        });

        let rendered = format_vitals_text(&data);

        assert_eq!(
            rendered,
            "url: https://example.com\n\
ttfb: -  fcp: -  lcp: -  cls: 0  inp: -\n\
hydration: -  phases: 0  hydratedComponents: 0"
        );
    }

    #[test]
    fn test_format_with_boundaries_applies_max_output() {
        let opts = OutputOptions {
            max_output: Some(5),
            ..OutputOptions::default()
        };

        let rendered = format_with_boundaries("abcdef", Some("https://example.com"), &opts);

        assert!(rendered.starts_with("abcde\n[truncated: showing 5 of 6 chars."));
    }

    #[test]
    fn test_format_with_boundaries_wraps_content() {
        let opts = OutputOptions {
            content_boundaries: true,
            ..OutputOptions::default()
        };

        let rendered = format_with_boundaries("content", Some("https://example.com"), &opts);

        assert!(rendered.contains("AGENT_BROWSER_PAGE_CONTENT"));
        assert!(rendered.contains("origin=https://example.com"));
        assert!(rendered.contains("\ncontent\n"));
        assert!(rendered.contains("END_AGENT_BROWSER_PAGE_CONTENT"));
    }

    #[test]
    fn test_boundary_origin_supports_read_metadata() {
        assert_eq!(
            boundary_origin(&json!({
                "finalUrl": "https://example.com/read",
                "url": "https://example.com/source"
            })),
            Some("https://example.com/read")
        );
        assert_eq!(
            boundary_origin(&json!({
                "origin": "https://example.com/dom",
                "finalUrl": "https://example.com/read"
            })),
            Some("https://example.com/dom")
        );
        assert_eq!(
            boundary_origin(&json!({ "url": "https://example.com/source" })),
            Some("https://example.com/source")
        );
    }
}
