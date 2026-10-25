//! Local users. "Administrator" means membership of `wheel`, which openSUSE's
//! sudo policy uses for admin rights.

use anyhow::{bail, Result};
use yaist_core::shell_quote as sq;
use yaist_core::{validate, ActionSpec, Context, ModuleSpec, Output, Param, Registry, Risk, Table};

const ADMIN_GROUP: &str = "wheel";

const LIST: ActionSpec = ActionSpec {
    id: "users.list",
    title: "List users",
    description: "List local human user accounts (UID 1000 and above) and whether they are administrators.",
    risk: Risk::Read,
    params: &[],
};

const CREATE: ActionSpec = ActionSpec {
    id: "users.create",
    title: "Add user",
    description: "Create a local user account with a home directory. The account stays locked until a password is set.",
    risk: Risk::Change,
    params: &[
        Param::text("username", "Login name, lowercase, e.g. alice").check(validate::username),
        Param::text("full_name", "Full name, e.g. Alice Smith").optional().check(validate::full_name),
        Param::boolean("admin", "Make the user an administrator (member of wheel, can use sudo)"),
    ],
};

const SET_ADMIN: ActionSpec = ActionSpec {
    id: "users.set_admin",
    title: "Admin rights",
    description: "Grant or revoke administrator rights (membership of wheel, sudo).",
    risk: Risk::Change,
    params: &[Param::text("username", "Login name").check(validate::username), Param::boolean("admin", "Administrator or not").required()],
};

const DELETE: ActionSpec = ActionSpec {
    id: "users.delete",
    title: "Delete user",
    description: "Delete a local user account, optionally with its home directory.",
    risk: Risk::Dangerous,
    params: &[
        Param::text("username", "Login name").check(validate::username),
        Param::boolean("remove_home", "Also delete the home directory and mail spool"),
    ],
};

pub fn register(reg: &mut Registry) {
    reg.module(ModuleSpec {
        id: "users",
        title: "Users",
        tagline: "Accounts and admin rights",
        overview: LIST.id,
        row_actions: &[SET_ADMIN.id, DELETE.id],
        overview_needs_admin: false,
    });

    reg.action(LIST, |_, _| async move { list() });

    reg.action(CREATE, |ctx, args| async move {
        let user = args.str("username");
        if lookup(user).is_some() {
            bail!("user {user} already exists");
        }
        let mut argv = vec!["/usr/sbin/useradd", "--create-home"];
        if let Some(name) = args.opt_str("full_name") {
            argv.extend(["--comment", name]);
        }
        if args.bool("admin") {
            argv.extend(["--groups", ADMIN_GROUP]);
        }
        argv.extend(["--", user]);
        ctx.run_privileged(&argv).await?;
        Ok(Output::Done(format!("Created user {user}; set a password with `passwd {user}` before logging in")))
    });

    reg.action(SET_ADMIN, |ctx, args| async move {
        let user = args.str("username");
        human_user(user)?;
        if args.bool("admin") {
            ctx.run_privileged(&["/usr/sbin/usermod", "--append", "--groups", ADMIN_GROUP, "--", user]).await?;
            Ok(Output::Done(format!("{user} is now an administrator")))
        } else {
            ctx.run_privileged(&["/usr/bin/gpasswd", "--delete", user, ADMIN_GROUP]).await?;
            Ok(Output::Done(format!("{user} is no longer an administrator")))
        }
    });

    reg.action(DELETE, |ctx, args| async move { delete(&ctx, args.str("username"), args.bool("remove_home")).await });

    reg.shell(LIST.id, |_, _| vec!["getent passwd".into(), format!("getent group {ADMIN_GROUP}")]);
    reg.shell(CREATE.id, |a, _| {
        let mut cmd = String::from("sudo useradd --create-home");
        if let Some(n) = a.opt_str("full_name") {
            cmd.push_str(&format!(" --comment {}", sq(n)));
        }
        if a.bool("admin") {
            cmd.push_str(&format!(" --groups {ADMIN_GROUP}"));
        }
        let user = sq(a.str("username"));
        vec![format!("{cmd} {user}"), format!("sudo passwd {user}")]
    });
    reg.shell(SET_ADMIN.id, |a, _| {
        let user = sq(a.str("username"));
        vec![if a.bool("admin") {
            format!("sudo usermod --append --groups {ADMIN_GROUP} {user}")
        } else {
            format!("sudo gpasswd --delete {user} {ADMIN_GROUP}")
        }]
    });
    reg.shell(DELETE.id, |a, _| vec![format!("sudo userdel{} {}", if a.bool("remove_home") { " --remove" } else { "" }, sq(a.str("username")))]);
}

struct User {
    name: String,
    uid: u32,
    gecos: String,
    home: String,
    shell: String,
}

fn passwd() -> Vec<User> {
    std::fs::read_to_string("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            (f.len() >= 7).then(|| User {
                name: f[0].into(),
                uid: f[2].parse().unwrap_or(0),
                gecos: f[4].split(',').next().unwrap_or("").into(),
                home: f[5].into(),
                shell: f[6].into(),
            })
        })
        .collect()
}

fn lookup(name: &str) -> Option<User> {
    passwd().into_iter().find(|u| u.name == name)
}

/// Refuses system accounts: YAiST only manages people's accounts.
fn human_user(name: &str) -> Result<User> {
    match lookup(name) {
        None => bail!("no such user: {name}"),
        Some(u) if !(1000..60000).contains(&u.uid) => bail!("{name} is a system account (UID {}); YAiST only manages regular users", u.uid),
        Some(u) => Ok(u),
    }
}

fn group_members(group: &str) -> Vec<String> {
    std::fs::read_to_string("/etc/group")
        .unwrap_or_default()
        .lines()
        .find(|l| l.split(':').next() == Some(group))
        .and_then(|l| l.split(':').nth(3))
        .map(|m| m.split(',').filter(|s| !s.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default()
}

fn list() -> Result<Output> {
    let admins = group_members(ADMIN_GROUP);
    let mut t =
        Table::new(&[("username", "User"), ("full_name", "Name"), ("uid", "UID"), ("is_admin", "Admin"), ("shell", "Shell"), ("home", "Home")]);
    for u in passwd().into_iter().filter(|u| (1000..60000).contains(&u.uid)) {
        let admin = if admins.contains(&u.name) { "yes" } else { "no" };
        t.push(vec![u.name, u.gecos, u.uid.to_string(), admin.into(), u.shell, u.home]);
    }
    Ok(Output::Table(t))
}

async fn delete(ctx: &Context, user: &str, remove_home: bool) -> Result<Output> {
    let u = human_user(user)?;
    if std::env::var("USER").is_ok_and(|me| me == u.name) {
        bail!("refusing to delete the account you are logged in as");
    }
    let mut argv = vec!["/usr/sbin/userdel"];
    if remove_home {
        argv.push("--remove");
    }
    argv.extend(["--", user]);
    ctx.run_privileged(&argv).await?;
    Ok(Output::Done(format!("Deleted user {user}{}", if remove_home { " and their home directory" } else { "" })))
}
