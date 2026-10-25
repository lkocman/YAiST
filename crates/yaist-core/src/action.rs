//! Action definitions. One [`ActionSpec`] is the single source for the LLM tool
//! schema, the generated panel form and the `/command` syntax.

use anyhow::{bail, Result};
use serde_json::{Map, Value};

/// How much an action can change the system. Decides whether it runs straight
/// away or waits for the user's confirmation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Risk {
    /// Only reads state; runs without asking.
    Read,
    /// Changes the system; needs confirmation.
    Change,
    /// Hard to undo or affects trust (removing users, adding repositories).
    Dangerous,
}

impl Risk {
    pub fn label(self) -> &'static str {
        match self {
            Risk::Read => "read",
            Risk::Change => "change",
            Risk::Dangerous => "dangerous",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ParamKind {
    Text,
    Integer,
    Bool,
    Choice(&'static [&'static str]),
}

/// Checks a text value beyond its type: user names, unit names, URLs.
pub type Validator = fn(&str) -> std::result::Result<(), String>;

#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub name: &'static str,
    pub kind: ParamKind,
    pub required: bool,
    pub help: &'static str,
    pub default: Option<&'static str>,
    pub validate: Option<Validator>,
    /// Never shown: masked on plan cards, in /commands and terminal
    /// equivalents (e.g. a registration code).
    pub secret: bool,
    /// Long text (error output, steps): a multi-line field in forms.
    pub multiline: bool,
}

impl Param {
    pub const fn text(name: &'static str, help: &'static str) -> Self {
        Self { name, kind: ParamKind::Text, required: true, help, default: None, validate: None, secret: false, multiline: false }
    }
    pub const fn integer(name: &'static str, help: &'static str) -> Self {
        Self { name, kind: ParamKind::Integer, required: true, help, default: None, validate: None, secret: false, multiline: false }
    }
    pub const fn boolean(name: &'static str, help: &'static str) -> Self {
        Self { name, kind: ParamKind::Bool, required: false, help, default: Some("false"), validate: None, secret: false, multiline: false }
    }
    pub const fn choice(name: &'static str, options: &'static [&'static str], help: &'static str) -> Self {
        Self { name, kind: ParamKind::Choice(options), required: true, help, default: None, validate: None, secret: false, multiline: false }
    }
    pub const fn required(mut self) -> Self {
        self.required = true;
        self.default = None;
        self
    }
    pub const fn optional(mut self) -> Self {
        self.required = false;
        self
    }
    pub const fn default(mut self, value: &'static str) -> Self {
        self.required = false;
        self.default = Some(value);
        self
    }
    pub const fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }
    pub const fn secret(mut self) -> Self {
        self.secret = true;
        self
    }
    pub const fn check(mut self, validate: Validator) -> Self {
        self.validate = Some(validate);
        self
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ActionSpec {
    /// `module.verb`, e.g. `firewall.open_port`.
    pub id: &'static str,
    /// Short button label, e.g. "Open port".
    pub title: &'static str,
    /// What the action does; shown to the LLM and as a tooltip.
    pub description: &'static str,
    pub risk: Risk,
    pub params: &'static [Param],
}

impl ActionSpec {
    pub fn module(&self) -> &'static str {
        self.id.split_once('.').map_or(self.id, |(m, _)| m)
    }

    pub fn verb(&self) -> &'static str {
        self.id.split_once('.').map_or(self.id, |(_, v)| v)
    }

    pub fn param(&self, name: &str) -> Option<&Param> {
        self.params.iter().find(|p| p.name == name)
    }

    /// JSON schema of the arguments, for the LLM's tool definition.
    pub fn json_schema(&self) -> Value {
        let mut props = Map::new();
        let mut required = Vec::new();
        for p in self.params {
            let mut prop = match p.kind {
                ParamKind::Text => serde_json::json!({ "type": "string" }),
                ParamKind::Integer => serde_json::json!({ "type": "integer" }),
                ParamKind::Bool => serde_json::json!({ "type": "boolean" }),
                ParamKind::Choice(opts) => serde_json::json!({ "type": "string", "enum": opts }),
            };
            let mut help = p.help.to_owned();
            if let Some(d) = p.default {
                help.push_str(&format!(" (default: {d})"));
            }
            prop["description"] = Value::String(help);
            props.insert(p.name.to_owned(), prop);
            if p.required {
                required.push(Value::String(p.name.to_owned()));
            }
        }
        serde_json::json!({ "type": "object", "properties": props, "required": required })
    }

    /// Checks and normalises raw arguments, whoever sent them (LLM, form or
    /// `/command`). Strings are coerced to the declared type; unknown keys are
    /// rejected so a confused model can't smuggle anything through.
    pub fn validate(&self, raw: &Map<String, Value>) -> Result<Args> {
        for key in raw.keys() {
            if self.param(key).is_none() {
                bail!("{} has no parameter '{key}'", self.id);
            }
        }
        let mut out = Map::new();
        for p in self.params {
            let value = match raw.get(p.name) {
                Some(Value::Null) | None => match p.default {
                    Some(d) => Value::String(d.to_owned()),
                    None if p.required => bail!("missing required parameter '{}'", p.name),
                    None => continue,
                },
                Some(v) => v.clone(),
            };
            out.insert(p.name.to_owned(), coerce(p, value)?);
        }
        Ok(Args(out))
    }
}

fn coerce(p: &Param, value: Value) -> Result<Value> {
    let as_text = |v: &Value| match v {
        Value::String(s) => s.trim().to_owned(),
        other => other.to_string(),
    };
    Ok(match p.kind {
        ParamKind::Text => {
            let s = as_text(&value);
            if p.required && s.is_empty() {
                bail!("'{}' must not be empty", p.name);
            }
            if let Some(check) = p.validate {
                check(&s).map_err(|e| anyhow::anyhow!("invalid {}: {e}", p.name))?;
            }
            Value::String(s)
        }
        ParamKind::Integer => match &value {
            Value::Number(n) if n.is_i64() => value,
            _ => {
                let s = as_text(&value);
                let n: i64 = s.parse().map_err(|_| anyhow::anyhow!("'{}' must be a number, got '{s}'", p.name))?;
                Value::from(n)
            }
        },
        ParamKind::Bool => match &value {
            Value::Bool(_) => value,
            _ => match as_text(&value).to_ascii_lowercase().as_str() {
                "true" | "yes" | "y" | "1" | "on" => Value::Bool(true),
                "false" | "no" | "n" | "0" | "off" | "" => Value::Bool(false),
                other => bail!("'{}' must be true or false, got '{other}'", p.name),
            },
        },
        ParamKind::Choice(opts) => {
            let s = as_text(&value).to_ascii_lowercase();
            if !opts.contains(&s.as_str()) {
                bail!("'{}' must be one of {}, got '{s}'", p.name, opts.join(", "));
            }
            Value::String(s)
        }
    })
}

/// Validated arguments.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Args(pub Map<String, Value>);

impl Args {
    pub fn str(&self, name: &str) -> &str {
        self.0.get(name).and_then(Value::as_str).unwrap_or("")
    }
    pub fn opt_str(&self, name: &str) -> Option<&str> {
        self.0.get(name).and_then(Value::as_str).filter(|s| !s.is_empty())
    }
    pub fn int(&self, name: &str) -> i64 {
        self.0.get(name).and_then(Value::as_i64).unwrap_or(0)
    }
    pub fn bool(&self, name: &str) -> bool {
        self.0.get(name).and_then(Value::as_bool).unwrap_or(false)
    }
    /// The value for showing to the user: secrets are masked.
    pub fn shown(&self, spec: &ActionSpec, name: &str) -> String {
        if spec.param(name).is_some_and(|p| p.secret) && self.0.contains_key(name) {
            "••••••".into()
        } else {
            self.display(name)
        }
    }

    /// The value as the user would type it.
    pub fn display(&self, name: &str) -> String {
        match self.0.get(name) {
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
            None => String::new(),
        }
    }
}
