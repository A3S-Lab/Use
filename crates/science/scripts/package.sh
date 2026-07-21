#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
crate_dir="$(cd "${script_dir}/.." && pwd)"
workspace_dir="$(cd "${crate_dir}/../.." && pwd)"
output_dir="${1:-${crate_dir}/dist/a3s-use-science}"

if [[ -e "${output_dir}" ]]; then
  echo "refusing to overwrite existing output: ${output_dir}" >&2
  exit 2
fi

cargo build --manifest-path "${workspace_dir}/Cargo.toml" --release --locked -p a3s-use-science

target_dir="${CARGO_TARGET_DIR:-${workspace_dir}/target}"
mkdir -p "${output_dir}/bin" "${output_dir}/skills/a3s-use-science" "${output_dir}/web"
install -m 0755 "${target_dir}/release/a3s-use-science" "${output_dir}/bin/a3s-use-science"
install -m 0644 "${crate_dir}/package/a3s-use-extension.acl" "${output_dir}/a3s-use-extension.acl"
install -m 0644 "${crate_dir}/package/skills/a3s-use-science/SKILL.md" "${output_dir}/skills/a3s-use-science/SKILL.md"
install -m 0644 "${crate_dir}/package/web/activity.html" "${output_dir}/web/activity.html"
install -m 0644 "${workspace_dir}/LICENSE" "${output_dir}/LICENSE"
install -m 0644 "${crate_dir}/DATA_SOURCES.md" "${output_dir}/DATA_SOURCES.md"
install -m 0644 "${crate_dir}/UPSTREAM.md" "${output_dir}/UPSTREAM.md"

echo "packaged a3s/science at ${output_dir}"
