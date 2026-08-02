use a3s_use_mcp_release_fixture::render_mcp_release;
use anyhow::{bail, Context};

fn main() -> anyhow::Result<()> {
    let mut arguments = std::env::args().skip(1);
    let artifact_digest = arguments
        .next()
        .context("usage: a3s-use-mcp-release-descriptor <sha256> <size-bytes>")?;
    let artifact_size_bytes = arguments
        .next()
        .context("usage: a3s-use-mcp-release-descriptor <sha256> <size-bytes>")?
        .parse::<u64>()
        .context("artifact size must be an unsigned integer")?;
    if arguments.next().is_some() {
        bail!("usage: a3s-use-mcp-release-descriptor <sha256> <size-bytes>");
    }
    let rendered = render_mcp_release(artifact_digest, artifact_size_bytes)?;
    println!("{}", serde_json::to_string(&rendered)?);
    Ok(())
}
