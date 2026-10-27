//! The system modules. Each registers a [`yaist_core::ModuleSpec`] and its
//! actions; clients never call these directly, only through the registry.

pub mod community;
mod firewall;
mod flatpak;
mod helper;
mod media;
mod packages;
mod printers;
pub mod release_notes;
pub mod report;
mod repos;
mod selinux;
mod services;
mod snapshots;
mod system;
mod upgrade;
mod users;
mod zypper;

pub use packages::{update_counts, UpdateCounts};
use yaist_core::{ModuleSpec, Registry};

/// All modules, in sidebar order.
pub fn registry() -> Registry {
    let mut reg = Registry::default();
    // The start screen; clients draw it themselves around the system overview.
    reg.module(ModuleSpec {
        id: "welcome",
        title: "Welcome",
        tagline: "Start here",
        overview: "system.overview",
        row_actions: &[],
        overview_needs_admin: false,
    });
    community::register(&mut reg);
    report::register(&mut reg);
    system::register(&mut reg);
    users::register(&mut reg);
    services::register(&mut reg);
    firewall::register(&mut reg);
    selinux::register(&mut reg);
    packages::register(&mut reg);
    repos::register(&mut reg);
    media::register(&mut reg);
    printers::register(&mut reg);
    snapshots::register(&mut reg);
    upgrade::register(&mut reg);
    release_notes::register(&mut reg);
    reg
}
