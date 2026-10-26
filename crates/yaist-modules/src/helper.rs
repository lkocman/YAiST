//! Runs yaist-helper (the privileged half) through pkexec.

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use yaist_core::Context;

/// `$YAIST_HELPER`, the installed locations, or next to the running binary
/// (a development build). /usr/local first, so scripts/dev-install-helper.sh
/// wins over the package.
fn path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("YAIST_HELPER") {
        return Ok(p.into());
    }
    let installed = ["/usr/local/libexec/yaist/yaist-helper", "/usr/libexec/yaist/yaist-helper"].map(PathBuf::from);
    let sibling = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("yaist-helper")));
    installed.into_iter().chain(sibling).find(|p| p.is_file()).ok_or_else(|| anyhow!("yaist-helper not found (build it, or set YAIST_HELPER)"))
}

pub async fn run(ctx: &Context, args: &[&str]) -> Result<String> {
    let helper = path()?;
    let helper = helper.to_str().ok_or_else(|| anyhow!("helper path is not UTF-8"))?;
    let mut argv = vec![helper];
    argv.extend_from_slice(args);
    ctx.run_privileged(&argv).await
}
