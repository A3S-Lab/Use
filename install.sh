#!/bin/sh

set -eu

repository="A3S-Lab/Use"
version="${A3S_USE_VERSION:-}"
base_url="${A3S_USE_RELEASE_BASE_URL:-https://github.com/${repository}/releases/download}"
install_root="${A3S_USE_INSTALL_ROOT:-}"
bin_dir="${A3S_USE_BIN_DIR:-}"
cosign_command="${A3S_USE_COSIGN:-cosign}"
download_root=""
stage_root=""
install_lock=""

usage() {
  cat <<'EOF'
Install a verified A3S Use release archive.

Usage: install.sh [options]

Options:
  --version <version>       Release version, with or without a leading v
  --base-url <url>          Release download root (default: GitHub Releases)
  --install-root <path>     Versioned installation root
  --bin-dir <path>          Directory for the managed a3s-use symlink
  --cosign <command>        Cosign executable (default: cosign from PATH)
  -h, --help                Show this help

Environment equivalents:
  A3S_USE_VERSION
  A3S_USE_RELEASE_BASE_URL
  A3S_USE_INSTALL_ROOT
  A3S_USE_BIN_DIR
  A3S_USE_COSIGN
EOF
}

fail() {
  printf 'a3s-use installer: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  set +e
  if [ -n "${stage_root}" ] && [ -d "${stage_root}" ]; then
    rm -rf -- "${stage_root}"
  fi
  if [ -n "${download_root}" ] && [ -d "${download_root}" ]; then
    rm -rf -- "${download_root}"
  fi
  if [ -n "${install_lock}" ] && [ -d "${install_lock}" ]; then
    rm -f -- "${install_lock}/pid"
    rmdir "${install_lock}" 2>/dev/null || true
  fi
}

trap cleanup 0 1 2 3 15

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      [ "$#" -ge 2 ] || fail "--version requires a value"
      version=$2
      shift 2
      ;;
    --base-url)
      [ "$#" -ge 2 ] || fail "--base-url requires a value"
      base_url=$2
      shift 2
      ;;
    --install-root)
      [ "$#" -ge 2 ] || fail "--install-root requires a value"
      install_root=$2
      shift 2
      ;;
    --bin-dir)
      [ "$#" -ge 2 ] || fail "--bin-dir requires a value"
      bin_dir=$2
      shift 2
      ;;
    --cosign)
      [ "$#" -ge 2 ] || fail "--cosign requires a value"
      cosign_command=$2
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "unknown option: $1"
      ;;
  esac
done

for command_name in curl tar awk diff find mktemp cp; do
  command -v "${command_name}" >/dev/null 2>&1 || fail "${command_name} is required"
done

case "${cosign_command}" in
  /*)
    cosign_path=${cosign_command}
    ;;
  *)
    cosign_path=$(command -v "${cosign_command}" 2>/dev/null) || \
      fail "Cosign is required to verify release signatures; install cosign or pass --cosign"
    ;;
esac
[ -f "${cosign_path}" ] && [ -x "${cosign_path}" ] || \
  fail "Cosign is required to verify release signatures; install cosign or pass --cosign"

if [ -z "${install_root}" ] || [ -z "${bin_dir}" ]; then
  [ -n "${HOME:-}" ] || fail "HOME or explicit installation paths are required"
  if [ -z "${install_root}" ]; then
    install_root="${XDG_DATA_HOME:-${HOME}/.local/share}/a3s-use"
  fi
  if [ -z "${bin_dir}" ]; then
    bin_dir="${HOME}/.local/bin"
  fi
fi

case "${install_root}" in
  /*) ;;
  *) fail "--install-root must be an absolute path" ;;
esac
case "${bin_dir}" in
  /*) ;;
  *) fail "--bin-dir must be an absolute path" ;;
esac

base_url=${base_url%/}
case "${base_url}" in
  *\?*|*\#*) fail "--base-url cannot contain a query or fragment" ;;
esac
case "${base_url#*://}" in
  *@*) fail "--base-url cannot contain credentials" ;;
esac

validate_download_url() {
  case "$1" in
    https://*) ;;
    http://127.0.0.1:*|http://localhost:*) ;;
    *) fail "downloads require HTTPS; plain HTTP is allowed only for a loopback test server" ;;
  esac
}

download_file() {
  destination=$1
  source_url=$2
  validate_download_url "${source_url}"
  case "${source_url}" in
    https://*)
      curl --proto '=https' --proto-redir '=https' --tlsv1.2 \
        --fail --silent --show-error --location \
        --retry 3 --retry-all-errors --output "${destination}" "${source_url}"
      ;;
    *)
      curl --proto '=http' --max-redirs 0 --fail --silent --show-error --retry 3 \
        --output "${destination}" "${source_url}"
      ;;
  esac
}

download_root=$(mktemp -d "${TMPDIR:-/tmp}/a3s-use-install.XXXXXX")

if [ -z "${version}" ]; then
  [ "${base_url}" = "https://github.com/${repository}/releases/download" ] || \
    fail "--version is required with a custom --base-url"
  latest_url="https://github.com/${repository}/releases/latest"
  validate_download_url "${latest_url}"
  effective_url=$(curl --proto '=https' --proto-redir '=https' --tlsv1.2 \
    --fail --silent --show-error \
    --location --retry 3 --retry-all-errors --output /dev/null \
    --write-out '%{url_effective}' "${latest_url}")
  version=${effective_url##*/}
fi

case "${version}" in
  v*) version=${version#v} ;;
esac
printf '%s\n' "${version}" | grep -Eq \
  '^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z][0-9A-Za-z.-]*)?$' || \
  fail "release version is not a supported semantic version: ${version}"
tag="v${version}"

system=$(uname -s)
machine=$(uname -m)
case "${system}:${machine}" in
  Linux:x86_64|Linux:amd64) platform="linux-x86_64" ;;
  Linux:aarch64|Linux:arm64) platform="linux-arm64" ;;
  Darwin:x86_64|Darwin:amd64) platform="darwin-x86_64" ;;
  Darwin:arm64|Darwin:aarch64) platform="darwin-arm64" ;;
  *) fail "unsupported platform: ${system} ${machine}" ;;
esac

archive_name="a3s-use-${version}-${platform}.tar.gz"
archive_path="${download_root}/${archive_name}"
checksums_path="${download_root}/checksums.txt"
signature_bundle_path="${download_root}/checksums.txt.sigstore.json"
release_url="${base_url}/${tag}"

download_file "${checksums_path}" "${release_url}/checksums.txt"
download_file "${signature_bundle_path}" "${release_url}/checksums.txt.sigstore.json"
certificate_identity="https://github.com/${repository}/.github/workflows/release.yml@refs/tags/${tag}"
if ! "${cosign_path}" verify-blob \
  --bundle "${signature_bundle_path}" \
  --certificate-identity "${certificate_identity}" \
  --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
  "${checksums_path}" >/dev/null 2>&1; then
  fail "Sigstore verification failed for checksums.txt"
fi
download_file "${archive_path}" "${release_url}/${archive_name}"

expected_matches=$(awk -v name="${archive_name}" \
  '$2 == name || $2 == "*" name { print $1 }' "${checksums_path}")
match_count=$(printf '%s\n' "${expected_matches}" | awk 'NF { count += 1 } END { print count + 0 }')
[ "${match_count}" -eq 1 ] || fail "checksums.txt must contain exactly one entry for ${archive_name}"
expected_sha256=$(printf '%s\n' "${expected_matches}" | tr 'A-F' 'a-f')
case "${expected_sha256}" in
  ''|*[!0-9a-f]*) fail "the published SHA-256 is malformed" ;;
esac
[ "${#expected_sha256}" -eq 64 ] || fail "the published SHA-256 is malformed"

if command -v sha256sum >/dev/null 2>&1; then
  actual_sha256=$(sha256sum "${archive_path}" | awk '{ print $1 }')
elif command -v shasum >/dev/null 2>&1; then
  actual_sha256=$(shasum -a 256 "${archive_path}" | awk '{ print $1 }')
elif command -v openssl >/dev/null 2>&1; then
  actual_sha256=$(openssl dgst -sha256 "${archive_path}" | awk '{ print $NF }')
else
  fail "sha256sum, shasum, or openssl is required"
fi
actual_sha256=$(printf '%s\n' "${actual_sha256}" | tr 'A-F' 'a-f')
[ "${actual_sha256}" = "${expected_sha256}" ] || \
  fail "SHA-256 mismatch for ${archive_name}"

if tar tzf "${archive_path}" | awk '
  /^\// || /^\.\.($|\/)/ || /\/\.\.($|\/)/ || /\\/ { unsafe = 1 }
  END { exit unsafe ? 0 : 1 }
'; then
  fail "the release archive contains an unsafe path"
fi
if tar tvzf "${archive_path}" | awk '
  substr($1, 1, 1) != "-" && substr($1, 1, 1) != "d" { special = 1 }
  END { exit special ? 0 : 1 }
'; then
  fail "the release archive contains a link or special file"
fi

releases_root="${install_root}/releases"
release_root="${releases_root}/${version}"
mkdir -p "${install_root}"
[ ! -L "${install_root}" ] || fail "the installation root cannot be a symbolic link"
install_lock="${install_root}/.install.lock"
if ! mkdir "${install_lock}" 2>/dev/null; then
  fail "another installation is active; remove ${install_lock} only after confirming no installer is running"
fi
printf '%s\n' "$$" > "${install_lock}/pid"

mkdir -p "${bin_dir}"
shim="${bin_dir}/a3s-use"
if [ -e "${shim}" ] && [ ! -L "${shim}" ]; then
  fail "refusing to replace non-symlink command: ${shim}"
fi
if [ -L "${shim}" ]; then
  existing_target=$(readlink "${shim}")
  case "${existing_target}" in
    "${install_root}/releases/"*) ;;
    *) fail "refusing to replace a symlink not managed by A3S Use: ${shim}" ;;
  esac
fi

mkdir -p "${releases_root}"
[ ! -L "${releases_root}" ] || fail "the releases directory cannot be a symbolic link"
stage_root=$(mktemp -d "${releases_root}/.stage-${version}.XXXXXX")
tar xzf "${archive_path}" -C "${stage_root}"

[ -f "${stage_root}/a3s-use" ] && [ -x "${stage_root}/a3s-use" ] || \
  fail "the release archive does not contain an executable a3s-use"
[ -f "${stage_root}/a3s-use-browser-driver" ] && \
  [ -x "${stage_root}/a3s-use-browser-driver" ] || \
  fail "the release archive does not contain an executable Browser driver"
[ ! -L "${stage_root}/a3s-use" ] && [ ! -L "${stage_root}/a3s-use-browser-driver" ] || \
  fail "release executables cannot be links"
for required_file in \
  "skills/a3s-use-browser/SKILL.md" \
  "skill-data/core/SKILL.md" \
  "ocr-skills/a3s-use-ocr/SKILL.md" \
  "ocr-models/PP-OCRv6_small/det/inference.onnx" \
  "ocr-models/PP-OCRv6_small/det/inference.yml" \
  "ocr-models/PP-OCRv6_small/rec/inference.onnx" \
  "ocr-models/PP-OCRv6_small/rec/inference.yml" \
  "dashboard/index.html"
do
  [ -f "${stage_root}/${required_file}" ] && [ ! -L "${stage_root}/${required_file}" ] || \
    fail "the release archive is missing required file: ${required_file}"
done

cat > "${stage_root}/a3s-use-launcher" <<'EOF'
#!/bin/sh
set -eu

launcher_path=$0
case "${launcher_path}" in
  */*) ;;
  *) launcher_path=$(command -v "${launcher_path}") ;;
esac
if [ -L "${launcher_path}" ]; then
  launcher_target=$(readlink "${launcher_path}")
  case "${launcher_target}" in
    /*) ;;
    *) launcher_target=$(dirname "${launcher_path}")/${launcher_target} ;;
  esac
else
  launcher_target=${launcher_path}
fi
release_root=$(CDPATH= cd -P "$(dirname "${launcher_target}")" && pwd)

if [ -z "${A3S_USE_OCR_HOME:-}" ]; then
  A3S_USE_OCR_HOME=${release_root}/ocr-models
  export A3S_USE_OCR_HOME
fi
if [ -z "${A3S_USE_OCR_SKILLS_DIR:-}" ]; then
  A3S_USE_OCR_SKILLS_DIR=${release_root}/ocr-skills
  export A3S_USE_OCR_SKILLS_DIR
fi
if [ -z "${A3S_USE_BROWSER_SKILLS_DIR:-}" ]; then
  A3S_USE_BROWSER_SKILLS_DIR=${release_root}/skill-data
  export A3S_USE_BROWSER_SKILLS_DIR
fi

exec "${release_root}/a3s-use" "$@"
EOF
chmod 0755 "${stage_root}/a3s-use-launcher"
printf '%s\n' "${expected_sha256}" > "${stage_root}/.a3s-use-archive.sha256"
cp "${checksums_path}" "${stage_root}/.a3s-use-checksums.txt"
cp "${signature_bundle_path}" "${stage_root}/.a3s-use-checksums.sigstore.json"
chmod 0644 \
  "${stage_root}/.a3s-use-archive.sha256" \
  "${stage_root}/.a3s-use-checksums.txt" \
  "${stage_root}/.a3s-use-checksums.sigstore.json"

if [ -L "${release_root}" ]; then
  fail "the existing release path cannot be a symbolic link: ${release_root}"
elif [ -e "${release_root}" ]; then
  installed_digest=""
  if [ -f "${release_root}/.a3s-use-archive.sha256" ]; then
    installed_digest=$(tr -d '\r\n' < "${release_root}/.a3s-use-archive.sha256")
  fi
  [ "${installed_digest}" = "${expected_sha256}" ] || \
    fail "${release_root} already exists with different or unverifiable content"
  if find "${release_root}" ! -type f ! -type d -print | grep -q .; then
    fail "${release_root} contains a link or special file"
  fi
  [ -x "${release_root}/a3s-use" ] && \
    [ -x "${release_root}/a3s-use-browser-driver" ] && \
    [ -x "${release_root}/a3s-use-launcher" ] || \
    fail "${release_root} has invalid executable permissions"
  diff -qr "${stage_root}" "${release_root}" >/dev/null || \
    fail "${release_root} does not match the verified release archive"
  rm -rf -- "${stage_root}"
  stage_root=""
else
  mv "${stage_root}" "${release_root}"
  stage_root=""
fi

temporary_shim="${bin_dir}/.a3s-use.$$"
[ ! -e "${temporary_shim}" ] && [ ! -L "${temporary_shim}" ] || \
  fail "temporary command path already exists: ${temporary_shim}"
ln -s "${release_root}/a3s-use-launcher" "${temporary_shim}"
mv -f "${temporary_shim}" "${shim}"

printf 'Installed A3S Use %s for %s\n' "${version}" "${platform}"
printf 'Verified checksum signature: %s\n' "${certificate_identity}"
printf 'Verified archive: sha256:%s\n' "${expected_sha256}"
printf 'Command: %s\n' "${shim}"
case ":${PATH:-}:" in
  *":${bin_dir}:"*) ;;
  *) printf 'Add %s to PATH to invoke a3s-use directly.\n' "${bin_dir}" ;;
esac
