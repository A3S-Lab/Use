use std::net::IpAddr;

use url::Url;

pub(crate) fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub(crate) fn valid_package_id(value: &str) -> bool {
    super::PluginPackageId::is_valid(value)
}

pub(super) fn valid_catalog_text(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && !value.chars().any(char::is_control)
        && value.trim() == value
}

pub(super) fn valid_tag(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z' | b'0'..=b'9'))
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.' | b'_')
        })
}

pub(super) fn valid_permission_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z' | b'0'..=b'9'))
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'.' | b'_' | b'/')
        })
        && !value
            .split('/')
            .any(|segment| matches!(segment, "" | "." | ".."))
}

pub(crate) fn valid_machine_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b':' | b'/' | b'@')
        })
}

pub(super) fn valid_portable_scope_path(value: &str) -> bool {
    value == "."
        || (!value.is_empty()
            && value.len() <= 1024
            && !value.starts_with('/')
            && !value.contains('\\')
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-')
            })
            && value
                .split('/')
                .all(|segment| !matches!(segment, "" | "." | "..")))
}

pub(super) fn valid_dns_name(value: &str) -> bool {
    if value.is_empty() || value.len() > 253 || value.ends_with('.') {
        return false;
    }
    if value.parse::<IpAddr>().is_ok() {
        return true;
    }
    value.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    })
}

pub(super) fn valid_http_path(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= 1024
        && !value.contains("//")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'?' | b'#' | b'\\'))
        && !value
            .split('/')
            .any(|segment| matches!(segment, "." | ".."))
}

pub(crate) fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

pub(super) fn valid_repository_url(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    value.len() <= 2048
        && url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.as_str() == value
}

pub(super) fn valid_registry_url(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    let secure = url.scheme() == "https";
    let loopback_http = url.scheme() == "http"
        && url.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
    value.len() <= 2048
        && (secure || loopback_http)
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.as_str() == value
}

pub(super) fn valid_spdx_expression(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-+().: ".contains(&byte))
}

pub(super) fn valid_target(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

pub(super) fn valid_target_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 2048
        && !value.starts_with('/')
        && !value.contains('\\')
        && value.bytes().all(|byte| byte.is_ascii_graphic())
        && value
            .split('/')
            .all(|segment| !matches!(segment, "" | "." | ".."))
        && matches!(
            value.rsplit('/').next(),
            Some(name)
                if name.ends_with(".tar.gz")
                    || name.ends_with(".tgz")
                    || name.ends_with(".zip")
        )
}

pub(super) fn strictly_sorted_unique<T: Ord>(values: &[T]) -> bool {
    !values.windows(2).any(|pair| pair[0] >= pair[1])
}
