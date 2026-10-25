//! Local users. "Administrator" means membership of `wheel`, which openSUSE's
//! sudo policy uses for admin rights.

use anyhow::{bail, Result};
use yaist_core::shell_quote as sq;
use yaist_core::{validate, ActionSpec, Context, ModuleSpec, Output, Param, Registry, Risk, Table};

const ADMIN_GROUP: &str = "wheel";

const LIST: ActionSpec = ActionSpec {
    id: "users.list",
    title: "List users",
    description: "List local human user accounts (UID 1000 and above): whether they are administrators, and their groups.",
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

const GROUPS: ActionSpec = ActionSpec {
    id: "users.groups",
    title: "List groups",
    description: "List local groups with their GID and members, e.g. to choose one to add a user to.",
    risk: Risk::Read,
    params: &[],
};

const GROUP_PARAMS: &[Param] = &[
    Param::text("username", "Login name").check(validate::username),
    Param::text("group", "Group name, as listed by List groups").check(validate::name),
];

const ADD_TO_GROUP: ActionSpec = ActionSpec {
    id: "users.add_to_group",
    title: "Add to group",
    description: "Add a user to an existing group (supplementary membership; takes effect at the next login). Some groups give root-level access: wheel (sudo), docker, disk.",
    risk: Risk::Change,
    params: GROUP_PARAMS,
};

const REMOVE_FROM_GROUP: ActionSpec = ActionSpec {
    id: "users.remove_from_group",
    title: "Remove from group",
    description: "Remove a user from a group they are a supplementary member of (takes effect at the next login).",
    risk: Risk::Change,
    params: GROUP_PARAMS,
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
        tagline: "Accounts, groups and admin rights",
        overview: LIST.id,
        row_actions: &[SET_ADMIN.id, ADD_TO_GROUP.id, REMOVE_FROM_GROUP.id, DELETE.id],
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

    reg.action(GROUPS, |_, _| async move { groups() });
    reg.action(ADD_TO_GROUP, |ctx, args| async move {
        let (user, group) = (args.str("username"), args.str("group"));
        human_user(user)?;
        if !group_exists(group) {
            bail!("there is no group {group}; List groups shows the existing ones");
        }
        if group_members(group).iter().any(|m| m == user) {
            return Ok(Output::Done(format!("{user} is already in {group}")));
        }
        ctx.run_privileged(&["/usr/sbin/usermod", "--append", "--groups", group, "--", user]).await?;
        Ok(Output::Done(format!("Added {user} to {group}; it takes effect at their next login")))
    });
    reg.action(REMOVE_FROM_GROUP, |ctx, args| async move {
        let (user, group) = (args.str("username"), args.str("group"));
        human_user(user)?;
        if !group_members(group).iter().any(|m| m == user) {
            bail!("{user} is not a member of {group}");
        }
        ctx.run_privileged(&["/usr/bin/gpasswd", "--delete", user, group]).await?;
        Ok(Output::Done(format!("Removed {user} from {group}; it takes effect at their next login")))
    });

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
    reg.shell(GROUPS.id, |_, _| vec!["getent group".into()]);
    reg.shell(ADD_TO_GROUP.id, |a, _| vec![format!("sudo usermod --append --groups {} {}", sq(a.str("group")), sq(a.str("username")))]);
    reg.shell(REMOVE_FROM_GROUP.id, |a, _| vec![format!("sudo gpasswd --delete {} {}", sq(a.str("username")), sq(a.str("group")))]);
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

/// /etc/group: (name, gid, supplementary members).
fn group_file() -> Vec<(String, String, Vec<String>)> {
    std::fs::read_to_string("/etc/group")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            (f.len() >= 4).then(|| (f[0].to_owned(), f[2].to_owned(), f[3].split(',').filter(|s| !s.is_empty()).map(str::to_owned).collect()))
        })
        .collect()
}

fn group_exists(group: &str) -> bool {
    group_file().iter().any(|(name, ..)| name == group)
}

fn group_members(group: &str) -> Vec<String> {
    group_file().into_iter().find(|(name, ..)| name == group).map(|(.., members)| members).unwrap_or_default()
}

fn groups() -> Result<Output> {
    let mut t = Table::new(&[("group", "Group"), ("gid", "GID"), ("members", "Members")]);
    for (name, gid, members) in group_file() {
        t.push(vec![name, gid, members.join(", ")]);
    }
    Ok(Output::Table(t))
}

fn list() -> Result<Output> {
    let all_groups = group_file();
    let mut t = Table::new(&[
        ("username", "User"),
        ("full_name", "Name"),
        ("uid", "UID"),
        ("is_admin", "Admin"),
        ("groups", "Groups"),
        ("shell", "Shell"),
        ("home", "Home"),
    ])
    .essential(&["username", "full_name", "is_admin"]);
    for u in passwd().into_iter().filter(|u| (1000..60000).contains(&u.uid)) {
        let mine: Vec<&str> = all_groups.iter().filter(|(_, _, m)| m.contains(&u.name)).map(|(g, ..)| g.as_str()).collect();
        let admin = if mine.contains(&ADMIN_GROUP) { "yes" } else { "no" };
        t.push(vec![u.name.clone(), u.gecos, u.uid.to_string(), admin.into(), mine.join(", "), u.shell, u.home]);
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
