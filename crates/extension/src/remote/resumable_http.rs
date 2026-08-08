use std::time::Duration;

use a3s_use_core::{UseError, UseResult};
use reqwest::header::{ACCEPT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, LOCATION, RANGE};
use reqwest::{Response, StatusCode};
use tough::TargetName;
use url::Url;

use super::target_cache::ResumableTarget;
use super::validate_download_url;

const MAX_DOWNLOAD_ATTEMPTS: usize = 3;
const MAX_REDIRECTS: usize = 5;
const CHECKPOINT_BYTES: u64 = 8 * 1024 * 1024;

pub(super) fn target_url(
    targets_base_url: &Url,
    target_name: &TargetName,
    expected_sha256: &str,
    consistent_snapshot: bool,
) -> UseResult<Url> {
    let filename = if consistent_snapshot {
        format!("{expected_sha256}.{}", target_name.resolved())
    } else {
        target_name.resolved().to_owned()
    };
    let url = targets_base_url.join(&filename).map_err(|error| {
        download_error(
            "use.extension.registry_download_failed",
            format!("Failed to resolve the signed Registry target URL: {error}"),
        )
    })?;
    let base = if targets_base_url.as_str().ends_with('/') {
        targets_base_url.as_str().to_owned()
    } else {
        format!("{}/", targets_base_url.as_str())
    };
    if !url.as_str().starts_with(&base) {
        return Err(download_error(
            "use.extension.registry_download_failed",
            "The signed Registry target URL escapes its targets base URL.",
        ));
    }
    validate_download_url(&url)?;
    Ok(url)
}

pub(super) async fn download(
    target: &mut ResumableTarget,
    url: &Url,
    error_code: &'static str,
) -> UseResult<()> {
    if target.is_ready() {
        return Ok(());
    }
    let client = reqwest::Client::builder()
        .user_agent("a3s-use-extension/0.3")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| {
            download_error(
                error_code,
                format!("Failed to build the Registry target client: {error}"),
            )
        })?;

    let mut last_stream_error = None;
    for attempt in 0..MAX_DOWNLOAD_ATTEMPTS {
        if target.offset() == target.expected_length() {
            return target.commit(error_code).await;
        }
        let requested_offset = target.offset();
        let mut response = match send_request(&client, url, requested_offset, error_code).await {
            Ok(response) => response,
            Err(error) if attempt + 1 < MAX_DOWNLOAD_ATTEMPTS => {
                last_stream_error = Some(error.message);
                retry_delay(attempt).await;
                continue;
            }
            Err(error) => return Err(error),
        };
        if retryable_status(response.status()) && attempt + 1 < MAX_DOWNLOAD_ATTEMPTS {
            retry_delay(attempt).await;
            continue;
        }
        let response_offset = match validate_response(
            &response,
            requested_offset,
            target.expected_length(),
            error_code,
        ) {
            Ok(offset) => offset,
            Err(error) => {
                if response.status() == StatusCode::PARTIAL_CONTENT {
                    target.discard().await?;
                }
                return Err(error);
            }
        };
        if response_offset == 0 && requested_offset > 0 {
            target.reset().await?;
        }

        let mut checkpoint_bytes = 0_u64;
        loop {
            match response.chunk().await {
                Ok(Some(bytes)) => {
                    if target
                        .offset()
                        .checked_add(bytes.len() as u64)
                        .is_none_or(|length| length > target.expected_length())
                    {
                        target.discard().await?;
                        return Err(download_error(
                            error_code,
                            "The Registry target exceeds its signed length.",
                        ));
                    }
                    target.append(&bytes).await?;
                    checkpoint_bytes = checkpoint_bytes.saturating_add(bytes.len() as u64);
                    if checkpoint_bytes >= CHECKPOINT_BYTES {
                        target.checkpoint().await?;
                        checkpoint_bytes = 0;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    target.checkpoint().await?;
                    last_stream_error = Some(error.to_string());
                    break;
                }
            }
        }
        if target.offset() == target.expected_length() {
            return target.commit(error_code).await;
        }
        target.checkpoint().await?;
        if attempt + 1 < MAX_DOWNLOAD_ATTEMPTS {
            retry_delay(attempt).await;
        }
    }

    Err(download_error(
        error_code,
        last_stream_error.map_or_else(
            || "The Registry target ended before its signed length.".to_owned(),
            |error| format!("The Registry target stream was interrupted: {error}"),
        ),
    )
    .with_detail("resumeBytes", target.offset().to_string())
    .with_detail("expectedBytes", target.expected_length().to_string()))
}

async fn send_request(
    client: &reqwest::Client,
    initial_url: &Url,
    offset: u64,
    error_code: &'static str,
) -> UseResult<Response> {
    validate_download_url(initial_url)
        .map_err(|error| download_error(error_code, error.message))?;
    let require_https = initial_url.scheme() == "https";
    let mut url = initial_url.clone();
    for redirects in 0..=MAX_REDIRECTS {
        validate_redirect_url(&url, require_https, error_code)?;
        let mut request = client.get(url.clone()).header(ACCEPT_ENCODING, "identity");
        if offset > 0 {
            request = request.header(RANGE, format!("bytes={offset}-"));
        }
        let response = request.send().await.map_err(|error| {
            download_error(
                error_code,
                format!("Failed to request the signed Registry target: {error}"),
            )
        })?;
        if !response.status().is_redirection() {
            validate_redirect_url(response.url(), require_https, error_code)?;
            return Ok(response);
        }
        if redirects == MAX_REDIRECTS {
            return Err(download_error(
                error_code,
                "The Registry target exceeded the redirect limit.",
            ));
        }
        let location = response
            .headers()
            .get(LOCATION)
            .ok_or_else(|| {
                download_error(
                    error_code,
                    "The Registry target redirect has no Location header.",
                )
            })?
            .to_str()
            .map_err(|_| {
                download_error(
                    error_code,
                    "The Registry target redirect Location is not valid text.",
                )
            })?;
        url = response.url().join(location).map_err(|error| {
            download_error(
                error_code,
                format!("The Registry target redirect is invalid: {error}"),
            )
        })?;
    }
    Err(download_error(
        error_code,
        "The Registry target redirect state is invalid.",
    ))
}

fn validate_redirect_url(
    url: &Url,
    require_https: bool,
    error_code: &'static str,
) -> UseResult<()> {
    validate_download_url(url).map_err(|error| download_error(error_code, error.message))?;
    if require_https && url.scheme() != "https" {
        return Err(download_error(
            error_code,
            "An HTTPS Registry target cannot redirect to a non-HTTPS URL.",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(download_error(
            error_code,
            "Registry target URLs cannot contain credentials or fragments.",
        ));
    }
    Ok(())
}

fn validate_response(
    response: &Response,
    requested_offset: u64,
    expected_length: u64,
    error_code: &'static str,
) -> UseResult<u64> {
    match response.status() {
        StatusCode::OK => {
            validate_content_length(response, expected_length, error_code)?;
            Ok(0)
        }
        StatusCode::PARTIAL_CONTENT if requested_offset > 0 => {
            let remaining = expected_length
                .checked_sub(requested_offset)
                .ok_or_else(|| {
                    download_error(error_code, "The Registry target resume offset is invalid.")
                })?;
            validate_content_length(response, remaining, error_code)?;
            let value = response
                .headers()
                .get(CONTENT_RANGE)
                .ok_or_else(|| {
                    download_error(
                        error_code,
                        "The Registry target range response has no Content-Range header.",
                    )
                })?
                .to_str()
                .map_err(|_| {
                    download_error(
                        error_code,
                        "The Registry target Content-Range is not valid text.",
                    )
                })?;
            let expected = format!(
                "bytes {requested_offset}-{}/{}",
                expected_length - 1,
                expected_length
            );
            if value != expected {
                return Err(download_error(
                    error_code,
                    "The Registry target range response does not match the requested signed bytes.",
                )
                .with_detail("expectedContentRange", expected)
                .with_detail("actualContentRange", value.to_owned()));
            }
            Ok(requested_offset)
        }
        status => Err(download_error(
            error_code,
            format!("Registry target download returned HTTP {status}."),
        )),
    }
}

fn validate_content_length(
    response: &Response,
    expected: u64,
    error_code: &'static str,
) -> UseResult<()> {
    let Some(value) = response.headers().get(CONTENT_LENGTH) else {
        return Ok(());
    };
    let actual = value
        .to_str()
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            download_error(error_code, "The Registry target Content-Length is invalid.")
        })?;
    if actual != expected {
        return Err(download_error(
            error_code,
            "The Registry target Content-Length does not match its signed length.",
        )
        .with_detail("expectedLength", expected.to_string())
        .with_detail("actualLength", actual.to_string()));
    }
    Ok(())
}

fn retryable_status(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

async fn retry_delay(attempt: usize) {
    tokio::time::sleep(Duration::from_millis(100 * (attempt as u64 + 1))).await;
}

fn download_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consistent_target_url_matches_tuf_nested_path_rules() {
        let base = Url::parse("https://registry.example/targets/").unwrap();
        let name = TargetName::new("extensions/acme/tool/archive.tar.gz").unwrap();
        let digest = "a".repeat(64);

        assert_eq!(
            target_url(&base, &name, &digest, true).unwrap().as_str(),
            format!(
                "https://registry.example/targets/{digest}.extensions/acme/tool/archive.tar.gz"
            )
        );
        assert_eq!(
            target_url(&base, &name, &digest, false).unwrap().as_str(),
            "https://registry.example/targets/extensions/acme/tool/archive.tar.gz"
        );
    }
}
