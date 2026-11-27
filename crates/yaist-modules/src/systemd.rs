//! The bit of systemd other modules need: enabling a unit over D-Bus.
//! systemd checks polkit itself, so this asks for authentication through
//! the desktop's polkit agent.

use anyhow::Result;
use yaist_core::Context;
use zbus::OwnedObjectPath;

const SYSTEMD: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";

/// Enables a unit at boot (`systemctl enable [--now]`).
pub(crate) async fn enable(ctx: &Context, unit: &str, now: bool) -> Result<()> {
    ctx.call::<_, (bool, Vec<(String, String, String)>)>(SYSTEMD, SYSTEMD_PATH, MANAGER, "EnableUnitFiles", &(vec![unit], false, false), true)
        .await?;
    ctx.call::<_, ()>(SYSTEMD, SYSTEMD_PATH, MANAGER, "Reload", &(), true).await?;
    if now {
        ctx.call::<_, OwnedObjectPath>(SYSTEMD, SYSTEMD_PATH, MANAGER, "StartUnit", &(unit, "replace"), true).await?;
    }
    Ok(())
}
