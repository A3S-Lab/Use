#!/usr/bin/env bash
set -Eeuo pipefail

repository_root="$(git rev-parse --show-toplevel)"
if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
  echo "MCP release container conformance requires x86_64 Linux." >&2
  exit 2
fi
for command_name in awk cargo curl docker jq musl-gcc rustup sha256sum; do
  if ! command -v "${command_name}" >/dev/null 2>&1; then
    echo "Missing required command: ${command_name}" >&2
    exit 2
  fi
done
if ! docker info >/dev/null 2>&1; then
  echo "MCP release container conformance requires a running Docker daemon." >&2
  exit 2
fi
if ! docker buildx version >/dev/null 2>&1; then
  echo "MCP release container conformance requires Docker Buildx." >&2
  exit 2
fi

target="x86_64-unknown-linux-musl"
rustup target add "${target}"
cargo build \
  --manifest-path "${repository_root}/Cargo.toml" \
  --locked \
  --package a3s-use-mcp-release-fixture \
  --release \
  --target "${target}" \
  --bin a3s-use-mcp-release-fixture

staging_root="$(mktemp -d /tmp/a3s-use-mcp-release.XXXXXX)"
builder_name="a3s-use-mcp-builder-$$"
registry_name="a3s-use-mcp-registry-$$"
service_names=()
cleanup() {
  for service_name in "${service_names[@]}"; do
    docker rm --force "${service_name}" >/dev/null 2>&1 || true
  done
  docker buildx rm "${builder_name}" >/dev/null 2>&1 || true
  docker rm --force "${registry_name}" >/dev/null 2>&1 || true
  rm -rf "${staging_root}"
}
trap cleanup EXIT

cp "${repository_root}/crates/mcp-release-fixture/Containerfile" "${staging_root}/Containerfile"
cp \
  "${repository_root}/target/${target}/release/a3s-use-mcp-release-fixture" \
  "${staging_root}/a3s-use-mcp-release-fixture"

docker run \
  --detach \
  --name "${registry_name}" \
  --publish 127.0.0.1::5000 \
  registry:2.8.3 >/dev/null
registry_port="$(docker port "${registry_name}" 5000/tcp | sed -n 's/.*://p' | head -n 1)"
if [[ ! "${registry_port}" =~ ^[0-9]+$ ]]; then
  echo "Failed to resolve the temporary Registry port." >&2
  exit 1
fi
registry_endpoint="http://127.0.0.1:${registry_port}"
for _ in {1..100}; do
  if curl --fail --silent "${registry_endpoint}/v2/" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done
curl --fail --silent --show-error "${registry_endpoint}/v2/" >/dev/null

docker buildx create \
  --name "${builder_name}" \
  --driver docker-container \
  --driver-opt network=host >/dev/null
docker buildx inspect "${builder_name}" --bootstrap >/dev/null

image_tag="127.0.0.1:${registry_port}/a3s/mcp-release-fixture:conformance"
docker buildx build \
  --builder "${builder_name}" \
  --file "${staging_root}/Containerfile" \
  --output "type=image,name=${image_tag},push=true,oci-mediatypes=true" \
  --platform linux/amd64 \
  --provenance=false \
  --sbom=false \
  "${staging_root}" >/dev/null

manifest_headers="${staging_root}/manifest.headers"
manifest_body="${staging_root}/manifest.json"
curl \
  --fail \
  --silent \
  --show-error \
  --header 'Accept: application/vnd.oci.image.manifest.v1+json' \
  --dump-header "${manifest_headers}" \
  --output "${manifest_body}" \
  "${registry_endpoint}/v2/a3s/mcp-release-fixture/manifests/conformance"
manifest_media_type="$(
  awk 'tolower($1) == "content-type:" { gsub("\r", "", $2); print $2; exit }' \
    "${manifest_headers}"
)"
artifact_digest="$(
  awk 'tolower($1) == "docker-content-digest:" { gsub("\r", "", $2); print $2; exit }' \
    "${manifest_headers}"
)"
if [[ "${manifest_media_type}" != "application/vnd.oci.image.manifest.v1+json" ]]; then
  echo "Registry returned non-OCI MCP fixture media type: ${manifest_media_type}" >&2
  exit 1
fi
if [[ ! "${artifact_digest}" =~ ^sha256:[0-9a-f]{64}$ ]]; then
  echo "The pushed MCP fixture did not resolve to one immutable manifest digest." >&2
  exit 1
fi
computed_digest="sha256:$(sha256sum "${manifest_body}" | awk '{ print $1 }')"
if [[ "${computed_digest}" != "${artifact_digest}" ]]; then
  echo "Registry MCP manifest bytes do not match its advertised digest." >&2
  exit 1
fi
artifact_size="$(wc -c <"${manifest_body}" | tr -d '[:space:]')"
if [[ ! "${artifact_size}" =~ ^[1-9][0-9]*$ ]]; then
  echo "Registry MCP manifest has no positive exact byte size." >&2
  exit 1
fi
pinned_image="${image_tag%:*}@${artifact_digest}"
rendered_release="$(
  cargo run \
    --manifest-path "${repository_root}/Cargo.toml" \
    --quiet \
    --locked \
    --package a3s-use-mcp-release-fixture \
    --bin a3s-use-mcp-release-descriptor \
    -- "${artifact_digest}" "${artifact_size}"
)"
release_identity="$(jq -er '.descriptorDigest' <<<"${rendered_release}")"
rendered_artifact="$(jq -er '.descriptor.artifact.digest' <<<"${rendered_release}")"
rendered_media_type="$(jq -er '.descriptor.artifact.mediaType' <<<"${rendered_release}")"
rendered_size="$(jq -er '.descriptor.artifact.sizeBytes' <<<"${rendered_release}")"
if [[ "${rendered_artifact}" != "${artifact_digest}" ||
      "${rendered_media_type}" != "${manifest_media_type}" ||
      "${rendered_size}" != "${artifact_size}" ]]; then
  echo "Rendered release descriptor does not bind the exact pushed OCI manifest." >&2
  exit 1
fi

for generation in 1 2; do
  service_name="a3s-use-mcp-fixture-${generation}-$$"
  service_names+=("${service_name}")
  docker run \
    --detach \
    --name "${service_name}" \
    --publish 127.0.0.1::8080 \
    --env "A3S_MCP_FIXTURE_RELEASE_IDENTITY=${release_identity}" \
    "${pinned_image}" >/dev/null
  service_port="$(docker port "${service_name}" 8080/tcp | sed -n 's/.*://p' | head -n 1)"
  if [[ ! "${service_port}" =~ ^[0-9]+$ ]]; then
    echo "Failed to resolve MCP fixture generation ${generation} port." >&2
    exit 1
  fi

  A3S_MCP_CONFORMANCE_ENDPOINT="http://127.0.0.1:${service_port}/mcp" \
  A3S_MCP_CONFORMANCE_RELEASE_IDENTITY="${release_identity}" \
    cargo test \
      --manifest-path "${repository_root}/Cargo.toml" \
      --quiet \
      --locked \
      --package a3s-use-mcp-release-fixture \
      --test headless_lifecycle \
      external_digest_pinned_container_conforms \
      -- --exact --nocapture

  docker stop --time 5 "${service_name}" >/dev/null
  if [[ "$(docker inspect "${service_name}" --format '{{.State.Running}}')" != "false" ]]; then
    echo "MCP fixture generation ${generation} exceeded its shutdown bound." >&2
    docker logs "${service_name}" >&2
    exit 1
  fi
  exit_code="$(docker inspect "${service_name}" --format '{{.State.ExitCode}}')"
  if [[ "${exit_code}" != "0" ]]; then
    echo "MCP fixture generation ${generation} exited with ${exit_code}." >&2
    docker logs "${service_name}" >&2
    exit 1
  fi
  docker rm "${service_name}" >/dev/null
done

echo "Digest-pinned MCP release conformance passed twice for ${artifact_digest}."
