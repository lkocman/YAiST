//! What clients render. The backend writes it; the TUI and GUI read it every
//! frame and keep only their own widget state (focus, input text, open form).

use std::time::Duration;

use yaist_core::ask::Question;
use yaist_core::{ActionSpec, Args, Distro, ModuleSpec, Output};

#[derive(Clone, Debug)]
pub enum Panel {
    Loading,
    /// Not loaded: reading it asks for the administrator password, so it
    /// waits for the user to ask (ModuleSpec::overview_needs_admin).
    Locked,
    Ready(Output),
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct ModuleView {
    pub spec: ModuleSpec,
    pub panel: Panel,
    /// Buttons above the table: the module's actions that are neither the
    /// overview nor row actions.
    pub actions: Vec<ActionSpec>,
    pub row_actions: Vec<ActionSpec>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlanStatus {
    Pending,
    Running,
    Done(String),
    Failed(String),
    Cancelled,
}

/// A proposed change, waiting for the user. Everything shown on the card is
/// rendered from `spec` and `args` by our code, never from model-written text.
#[derive(Clone, Debug)]
pub struct Plan {
    pub id: u64,
    pub spec: ActionSpec,
    pub args: Args,
    pub command: String,
    /// The same change done by hand in a terminal, for learning.
    pub shell: Vec<String>,
    /// What YAiST actually ran (commands and D-Bus calls), once done.
    pub executed: Vec<String>,
    pub status: PlanStatus,
    pub from_agent: bool,
}

#[derive(Clone, Debug)]
pub enum Entry {
    User(String),
    /// A command the user ran, typed or via a panel.
    Command(String),
    /// A read action the agent ran on its own.
    AgentRan {
        command: String,
        shell: Vec<String>,
        ok: bool,
    },
    Assistant {
        text: String,
        elapsed: Duration,
    },
    Plan(Plan),
    Result {
        title: String,
        /// The terminal equivalent of what produced this.
        shell: Vec<String>,
        output: Output,
    },
    Error(String),
    Notice(String),
}

/// A question from a running change (a package conflict, a license),
/// waiting for the user's answer.
#[derive(Clone, Debug)]
pub struct Asked {
    pub id: u64,
    pub question: Question,
}

/// The result of the background update check.
#[derive(Clone, Debug)]
pub enum UpdateStatus {
    Checking,
    Known { packages: usize, security: usize, reboot: bool, checked: std::time::Instant },
    Failed(String),
}

impl UpdateStatus {
    /// Updates waiting (packages, security patches), if known.
    pub fn pending(&self) -> Option<(usize, usize)> {
        match self {
            UpdateStatus::Known { packages, security, .. } => Some((*packages, *security)),
            _ => None,
        }
    }

    /// Installed changes wait for a reboot.
    pub fn reboot_needed(&self) -> bool {
        matches!(self, UpdateStatus::Known { reboot: true, .. })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LlmStatus {
    Checking,
    Ready(String),
    Offline,
}

pub struct State {
    pub distro: Distro,
    pub modules: Vec<ModuleView>,
    pub selected: usize,
    pub conversation: Vec<Entry>,
    /// What the backend is doing, e.g. "Thinking…".
    pub busy: Option<String>,
    pub llm: LlmStatus,
    pub llm_url: String,
    pub updates: UpdateStatus,
    pub question: Option<Asked>,
}

impl State {
    pub fn pending_plan(&self) -> Option<&Plan> {
        self.conversation.iter().rev().find_map(|e| match e {
            Entry::Plan(p) if p.status == PlanStatus::Pending => Some(p),
            _ => None,
        })
    }

    pub fn plan_mut(&mut self, id: u64) -> Option<&mut Plan> {
        self.conversation.iter_mut().rev().find_map(|e| match e {
            Entry::Plan(p) if p.id == id => Some(p),
            _ => None,
        })
    }

    pub fn module_index(&self, id: &str) -> Option<usize> {
        self.modules.iter().position(|m| m.spec.id == id)
    }
}
