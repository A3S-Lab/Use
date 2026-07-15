//! Typed, in-process Browser sessions and semantic element references.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use a3s_use_core::{Artifact, UseError, UseResult, UseSessionId};
use chromiumoxide::Page;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, OwnedSemaphorePermit, RwLock};
use url::Url;

use crate::pool::{browser_error, BrowserPool};
use crate::renderer::{apply_wait_condition, capture_screenshot};
use crate::WaitCondition;

const MAX_SNAPSHOT_TEXT_BYTES: usize = 50_000;
const MAX_SNAPSHOT_ELEMENTS: usize = 1_000;

const SNAPSHOT_SCRIPT: &str = r#"(() => {
  const visible = (element) => {
    const style = window.getComputedStyle(element);
    const rect = element.getBoundingClientRect();
    return style.visibility !== 'hidden' && style.display !== 'none' && rect.width > 0 && rect.height > 0;
  };
  const escape = (value) => window.CSS && CSS.escape
    ? CSS.escape(value)
    : value.replace(/[^a-zA-Z0-9_-]/g, (character) => `\\${character}`);
  const selectorFor = (element) => {
    if (element.id) return `#${escape(element.id)}`;
    const parts = [];
    let current = element;
    while (current && current.nodeType === Node.ELEMENT_NODE && current !== document.documentElement) {
      let part = current.tagName.toLowerCase();
      const parent = current.parentElement;
      if (parent) {
        const siblings = Array.from(parent.children).filter((candidate) => candidate.tagName === current.tagName);
        if (siblings.length > 1) part += `:nth-of-type(${siblings.indexOf(current) + 1})`;
      }
      parts.unshift(part);
      current = parent;
    }
    return `html > ${parts.join(' > ')}`;
  };
  const roleFor = (element) => {
    if (element.getAttribute('role')) return element.getAttribute('role');
    const tag = element.tagName.toLowerCase();
    const type = (element.getAttribute('type') || '').toLowerCase();
    if (tag === 'a') return 'link';
    if (tag === 'button' || (tag === 'input' && ['button', 'submit', 'reset'].includes(type))) return 'button';
    if (tag === 'textarea' || (tag === 'input' && !['checkbox', 'radio', 'range', 'color', 'file'].includes(type))) return 'textbox';
    if (tag === 'select') return 'combobox';
    if (tag === 'input' && type === 'checkbox') return 'checkbox';
    if (tag === 'input' && type === 'radio') return 'radio';
    return tag;
  };
  const nameFor = (element) => (
    element.getAttribute('aria-label') ||
    element.getAttribute('title') ||
    element.getAttribute('placeholder') ||
    element.innerText ||
    element.value ||
    ''
  ).trim().slice(0, 500);
  const candidates = Array.from(document.querySelectorAll(
    'a[href],button,input:not([type="hidden"]),textarea,select,[role="button"],[role="link"],[contenteditable="true"]'
  )).filter(visible).slice(0, 1000);
  return {
    url: window.location.href,
    title: document.title || '',
    text: ((document.body && document.body.innerText) || '').slice(0, 50000),
    elements: candidates.map((element) => ({
      role: roleFor(element),
      name: nameFor(element),
      selector: selectorFor(element),
      value: typeof element.value === 'string' ? element.value.slice(0, 2000) : null,
      disabled: Boolean(element.disabled || element.getAttribute('aria-disabled') === 'true')
    }))
  };
})()"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSessionRequest {
    pub session: UseSessionId,
    pub url: Url,
    pub timeout_ms: u64,
    pub wait: WaitCondition,
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSessionInfo {
    pub session: UseSessionId,
    pub url: Url,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSnapshot {
    pub session: UseSessionId,
    pub generation: u64,
    pub url: Url,
    pub title: String,
    pub text: String,
    pub elements: Vec<SnapshotElement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotElement {
    pub reference: String,
    pub role: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserActionResult {
    pub session: UseSessionId,
    pub action: String,
    pub url: Url,
}

pub struct BrowserSessions {
    pool: Arc<BrowserPool>,
    sessions: RwLock<HashMap<UseSessionId, Arc<Mutex<SessionState>>>>,
    closed: AtomicBool,
}

struct SessionState {
    page: Page,
    generation: u64,
    references: HashMap<String, String>,
    _permit: OwnedSemaphorePermit,
}

impl BrowserSessions {
    pub fn new(pool: Arc<BrowserPool>) -> Self {
        Self {
            pool,
            sessions: RwLock::new(HashMap::new()),
            closed: AtomicBool::new(false),
        }
    }

    pub async fn open(&self, request: OpenSessionRequest) -> UseResult<BrowserSnapshot> {
        if self.closed.load(Ordering::Acquire) {
            return Err(sessions_closed_error());
        }
        validate_http_url(&request.url)?;
        if self.sessions.read().await.contains_key(&request.session) {
            return Err(UseError::new(
                "use.browser.session_exists",
                format!(
                    "Browser session '{}' already exists.",
                    request.session.as_str()
                ),
            ));
        }
        let timeout = std::time::Duration::from_millis(request.timeout_ms.max(1));
        match tokio::time::timeout(timeout, self.open_inner(request)).await {
            Ok(result) => result,
            Err(_) => Err(UseError::new(
                "use.browser.timeout",
                format!("Browser session open exceeded {} ms.", timeout.as_millis()),
            )),
        }
    }

    async fn open_inner(&self, request: OpenSessionRequest) -> UseResult<BrowserSnapshot> {
        let permit = Arc::clone(self.pool.tab_semaphore())
            .acquire_owned()
            .await
            .map_err(|error| browser_error(format!("Tab limit is closed: {error}")))?;
        let browser = self.pool.acquire_browser().await?;
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|error| browser_error(format!("Failed to open browser tab: {error}")))?;
        let mut opening_page = OpeningPageGuard::new(page);
        let page = opening_page.page()?;
        if let Some(user_agent) = request.user_agent.as_deref() {
            page.set_user_agent(
                chromiumoxide::cdp::browser_protocol::network::SetUserAgentOverrideParams::new(
                    user_agent,
                ),
            )
            .await
            .map_err(|error| browser_error(format!("Failed to set browser user agent: {error}")))?;
        }
        page.goto(request.url.as_str())
            .await
            .map_err(|error| browser_error(format!("Browser navigation failed: {error}")))?;
        apply_wait_condition(page, &request.wait).await?;
        let state = Arc::new(Mutex::new(SessionState {
            page: opening_page.take()?,
            generation: 0,
            references: HashMap::new(),
            _permit: permit,
        }));
        let snapshot = match snapshot_state(&request.session, &state).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                close_state(state).await;
                return Err(error);
            }
        };
        let mut sessions = self.sessions.write().await;
        if self.closed.load(Ordering::Acquire) || sessions.contains_key(&request.session) {
            let closed = self.closed.load(Ordering::Acquire);
            drop(sessions);
            close_state(state).await;
            return if closed {
                Err(sessions_closed_error())
            } else {
                Err(UseError::new(
                    "use.browser.session_exists",
                    format!(
                        "Browser session '{}' already exists.",
                        request.session.as_str()
                    ),
                ))
            };
        }
        sessions.insert(request.session, state);
        Ok(snapshot)
    }

    pub async fn list(&self) -> UseResult<Vec<BrowserSessionInfo>> {
        let states = self
            .sessions
            .read()
            .await
            .iter()
            .map(|(session, state)| (session.clone(), Arc::clone(state)))
            .collect::<Vec<_>>();
        let mut result = Vec::with_capacity(states.len());
        for (session, state) in states {
            let state = state.lock().await;
            result.push(BrowserSessionInfo {
                session,
                url: current_url(&state.page).await?,
            });
        }
        result.sort_by(|left, right| left.session.as_str().cmp(right.session.as_str()));
        Ok(result)
    }

    pub async fn navigate(
        &self,
        session: &UseSessionId,
        url: Url,
        wait: WaitCondition,
        timeout_ms: u64,
    ) -> UseResult<BrowserSnapshot> {
        validate_http_url(&url)?;
        let state = self.state(session).await?;
        let timeout = std::time::Duration::from_millis(timeout_ms.max(1));
        match tokio::time::timeout(timeout, async {
            let mut state = state.lock().await;
            invalidate_references(&mut state);
            state
                .page
                .goto(url.as_str())
                .await
                .map_err(|error| browser_error(format!("Browser navigation failed: {error}")))?;
            apply_wait_condition(&state.page, &wait).await?;
            snapshot_locked(session, &mut state).await
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(UseError::new(
                "use.browser.timeout",
                format!("Browser navigation exceeded {} ms.", timeout.as_millis()),
            )),
        }
    }

    pub async fn snapshot(&self, session: &UseSessionId) -> UseResult<BrowserSnapshot> {
        snapshot_state(session, &self.state(session).await?).await
    }

    pub async fn click(
        &self,
        session: &UseSessionId,
        reference: &str,
    ) -> UseResult<BrowserActionResult> {
        let state = self.state(session).await?;
        let mut state = state.lock().await;
        let selector = resolve_reference(&state, reference)?;
        invalidate_references(&mut state);
        let element =
            state.page.find_element(selector).await.map_err(|error| {
                browser_error(format!("Browser element lookup failed: {error}"))
            })?;
        element
            .click()
            .await
            .map_err(|error| browser_error(format!("Browser click failed: {error}")))?;
        action_result(session, "click", &state.page).await
    }

    pub async fn type_text(
        &self,
        session: &UseSessionId,
        reference: &str,
        text: &str,
    ) -> UseResult<BrowserActionResult> {
        let state = self.state(session).await?;
        let mut state = state.lock().await;
        let selector = resolve_reference(&state, reference)?;
        invalidate_references(&mut state);
        let element =
            state.page.find_element(selector).await.map_err(|error| {
                browser_error(format!("Browser element lookup failed: {error}"))
            })?;
        element
            .click()
            .await
            .map_err(|error| browser_error(format!("Browser focus failed: {error}")))?;
        element
            .type_str(text)
            .await
            .map_err(|error| browser_error(format!("Browser typing failed: {error}")))?;
        action_result(session, "type", &state.page).await
    }

    pub async fn press_key(
        &self,
        session: &UseSessionId,
        reference: &str,
        key: &str,
    ) -> UseResult<BrowserActionResult> {
        let state = self.state(session).await?;
        let mut state = state.lock().await;
        let selector = resolve_reference(&state, reference)?;
        invalidate_references(&mut state);
        let element =
            state.page.find_element(selector).await.map_err(|error| {
                browser_error(format!("Browser element lookup failed: {error}"))
            })?;
        element
            .click()
            .await
            .map_err(|error| browser_error(format!("Browser focus failed: {error}")))?;
        element
            .press_key(key)
            .await
            .map_err(|error| browser_error(format!("Browser key press failed: {error}")))?;
        action_result(session, "press", &state.page).await
    }

    pub async fn select(
        &self,
        session: &UseSessionId,
        reference: &str,
        value: &str,
    ) -> UseResult<BrowserActionResult> {
        let state = self.state(session).await?;
        let mut state = state.lock().await;
        let selector = resolve_reference(&state, reference)?;
        invalidate_references(&mut state);
        let selector = serde_json::to_string(&selector).map_err(snapshot_encoding_error)?;
        let value = serde_json::to_string(value).map_err(snapshot_encoding_error)?;
        let script = format!(
            "(() => {{ const element = document.querySelector({selector}); if (!element) throw new Error('element not found'); element.value = {value}; element.dispatchEvent(new Event('input', {{ bubbles: true }})); element.dispatchEvent(new Event('change', {{ bubbles: true }})); return true; }})()"
        );
        state
            .page
            .evaluate(script)
            .await
            .map_err(|error| browser_error(format!("Browser select failed: {error}")))?;
        action_result(session, "select", &state.page).await
    }

    pub async fn scroll(
        &self,
        session: &UseSessionId,
        x: i64,
        y: i64,
    ) -> UseResult<BrowserActionResult> {
        let state = self.state(session).await?;
        let mut state = state.lock().await;
        invalidate_references(&mut state);
        state
            .page
            .evaluate(format!("window.scrollBy({x}, {y}); true"))
            .await
            .map_err(|error| browser_error(format!("Browser scroll failed: {error}")))?;
        action_result(session, "scroll", &state.page).await
    }

    pub async fn screenshot(
        &self,
        session: &UseSessionId,
        path: impl AsRef<Path>,
    ) -> UseResult<Artifact> {
        let state = self.state(session).await?;
        let state = state.lock().await;
        capture_screenshot(&state.page, path.as_ref()).await
    }

    pub async fn close(&self, session: &UseSessionId) -> UseResult<bool> {
        let state = self.sessions.write().await.remove(session);
        let Some(state) = state else {
            return Ok(false);
        };
        close_state(state).await;
        Ok(true)
    }

    pub async fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        let states = self
            .sessions
            .write()
            .await
            .drain()
            .map(|(_, state)| state)
            .collect::<Vec<_>>();
        for state in states {
            close_state(state).await;
        }
        self.pool.shutdown().await;
    }

    async fn state(&self, session: &UseSessionId) -> UseResult<Arc<Mutex<SessionState>>> {
        self.sessions
            .read()
            .await
            .get(session)
            .cloned()
            .ok_or_else(|| {
                UseError::new(
                    "use.browser.session_missing",
                    format!("Browser session '{}' is not open.", session.as_str()),
                )
            })
    }
}

struct OpeningPageGuard {
    page: Option<Page>,
}

impl OpeningPageGuard {
    fn new(page: Page) -> Self {
        Self { page: Some(page) }
    }

    fn page(&self) -> UseResult<&Page> {
        self.page.as_ref().ok_or_else(|| {
            UseError::new(
                "use.browser.page_closed",
                "The Browser page closed before its session was activated.",
            )
        })
    }

    fn take(&mut self) -> UseResult<Page> {
        self.page.take().ok_or_else(|| {
            UseError::new(
                "use.browser.page_closed",
                "The Browser page closed before its session was activated.",
            )
        })
    }
}

impl Drop for OpeningPageGuard {
    fn drop(&mut self) {
        let Some(page) = self.page.take() else {
            return;
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = page.close().await;
            });
        }
    }
}

async fn snapshot_state(
    session: &UseSessionId,
    state: &Arc<Mutex<SessionState>>,
) -> UseResult<BrowserSnapshot> {
    let mut state = state.lock().await;
    snapshot_locked(session, &mut state).await
}

async fn snapshot_locked(
    session: &UseSessionId,
    state: &mut SessionState,
) -> UseResult<BrowserSnapshot> {
    let raw: RawSnapshot = state
        .page
        .evaluate(SNAPSHOT_SCRIPT)
        .await
        .map_err(|error| browser_error(format!("Browser snapshot failed: {error}")))?
        .into_value()
        .map_err(|error| browser_error(format!("Browser snapshot was invalid: {error}")))?;
    state.generation = state.generation.saturating_add(1);
    state.references.clear();
    let mut elements = Vec::with_capacity(raw.elements.len().min(MAX_SNAPSHOT_ELEMENTS));
    for (index, element) in raw
        .elements
        .into_iter()
        .take(MAX_SNAPSHOT_ELEMENTS)
        .enumerate()
    {
        let reference = format!("@e{}", index + 1);
        state.references.insert(reference.clone(), element.selector);
        elements.push(SnapshotElement {
            reference,
            role: element.role,
            name: element.name,
            value: element.value,
            disabled: element.disabled,
        });
    }
    let url = Url::parse(&raw.url)
        .map_err(|error| browser_error(format!("Browser returned an invalid URL: {error}")))?;
    Ok(BrowserSnapshot {
        session: session.clone(),
        generation: state.generation,
        url,
        title: raw.title,
        text: truncate_utf8(raw.text, MAX_SNAPSHOT_TEXT_BYTES),
        elements,
    })
}

fn resolve_reference(state: &SessionState, reference: &str) -> UseResult<String> {
    state.references.get(reference).cloned().ok_or_else(|| {
        UseError::new(
            "use.browser.reference_stale",
            format!(
                "Element reference '{reference}' is missing or stale for snapshot generation {}.",
                state.generation
            ),
        )
        .with_suggestion("Take a new browser snapshot and use one of its references.")
    })
}

fn invalidate_references(state: &mut SessionState) {
    state.references.clear();
}

async fn action_result(
    session: &UseSessionId,
    action: &str,
    page: &Page,
) -> UseResult<BrowserActionResult> {
    Ok(BrowserActionResult {
        session: session.clone(),
        action: action.to_string(),
        url: current_url(page).await?,
    })
}

async fn current_url(page: &Page) -> UseResult<Url> {
    let value = page
        .url()
        .await
        .map_err(|error| browser_error(format!("Failed to read Browser URL: {error}")))?
        .unwrap_or_else(|| "about:blank".to_string());
    Url::parse(&value).map_err(|error| browser_error(format!("Browser URL is invalid: {error}")))
}

async fn close_state(state: Arc<Mutex<SessionState>>) {
    let page = state.lock().await.page.clone();
    let _ = page.close().await;
}

fn validate_http_url(url: &Url) -> UseResult<()> {
    if matches!(url.scheme(), "http" | "https") {
        Ok(())
    } else {
        Err(UseError::new(
            "use.browser.url_scheme_unsupported",
            format!(
                "Browser sessions accept HTTP(S) URLs, not '{}'.",
                url.scheme()
            ),
        ))
    }
}

fn sessions_closed_error() -> UseError {
    UseError::new(
        "use.browser.sessions_closed",
        "The Browser session manager has already shut down.",
    )
}

fn truncate_utf8(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}

fn snapshot_encoding_error(error: serde_json::Error) -> UseError {
    browser_error(format!("Failed to encode Browser interaction: {error}"))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSnapshot {
    url: String,
    title: String,
    text: String,
    elements: Vec<RawSnapshotElement>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSnapshotElement {
    role: String,
    name: String,
    selector: String,
    value: Option<String>,
    disabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BrowserPoolConfig, BrowserProvider};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn snapshot_text_truncation_preserves_utf8_boundaries() {
        let value = format!("{}界", "a".repeat(MAX_SNAPSHOT_TEXT_BYTES - 1));
        let truncated = truncate_utf8(value, MAX_SNAPSHOT_TEXT_BYTES);
        assert_eq!(truncated.len(), MAX_SNAPSHOT_TEXT_BYTES - 1);
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn public_session_types_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<BrowserSessions>();
    }

    #[tokio::test]
    async fn shutdown_prevents_new_sessions_without_launching_a_provider() {
        let sessions =
            BrowserSessions::new(Arc::new(BrowserPool::new(BrowserPoolConfig::default())));
        sessions.shutdown().await;
        let error = sessions
            .open(OpenSessionRequest {
                session: UseSessionId::parse("after-shutdown").unwrap(),
                url: Url::parse("https://example.com").unwrap(),
                timeout_ms: 1_000,
                wait: WaitCondition::Load,
                user_agent: None,
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, "use.browser.sessions_closed");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn discovered_chrome_keeps_typed_session_state_when_available() {
        let Some(executable) = crate::detect_chrome() else {
            return;
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let fixture = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = vec![0; 4_096];
                let _ = stream.read(&mut request).await.unwrap();
                let body = r#"<!doctype html><html><head><title>A3S fixture</title></head><body><label for="query">Query</label><input id="query" aria-label="Query"></body></html>"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.shutdown().await.unwrap();
            }
        });
        let pool = Arc::new(BrowserPool::new(BrowserPoolConfig {
            provider: BrowserProvider::ChromeExecutable(executable),
            ..BrowserPoolConfig::default()
        }));
        let sessions = BrowserSessions::new(pool);
        let session = UseSessionId::parse("typed-session").unwrap();
        let result: UseResult<()> = async {
            let opened = sessions
                .open(OpenSessionRequest {
                    session: session.clone(),
                    url: Url::parse(&format!("http://{address}/fixture")).unwrap(),
                    timeout_ms: 10_000,
                    wait: WaitCondition::Load,
                    user_agent: None,
                })
                .await?;
            assert_eq!(opened.generation, 1);
            assert_eq!(opened.title, "A3S fixture");
            let textbox = opened
                .elements
                .iter()
                .find(|element| element.role == "textbox" && element.name == "Query")
                .expect("the fixture textbox should have a semantic reference");
            let reference = textbox.reference.clone();

            sessions.type_text(&session, &reference, "a3s use").await?;
            let stale = sessions.type_text(&session, &reference, " stale").await;
            assert_eq!(stale.unwrap_err().code, "use.browser.reference_stale");

            let updated = sessions.snapshot(&session).await?;
            assert_eq!(updated.generation, 2);
            assert_eq!(
                updated
                    .elements
                    .iter()
                    .find(|element| element.role == "textbox")
                    .and_then(|element| element.value.as_deref()),
                Some("a3s use")
            );
            assert!(sessions.close(&session).await?);
            assert!(!sessions.close(&session).await?);
            Ok(())
        }
        .await;
        sessions.shutdown().await;
        fixture.abort();
        let _ = fixture.await;
        result.unwrap();
    }
}
