//! The backend behind every client: owns the registry, the agent and the
//! shared [`State`]. Clients send [`Cmd`]s and render the state.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use tokio::sync::{mpsc, oneshot};
use yaist_agent::{action_id, Agent, LlmClient, LlmConfig};
use yaist_core::ask::{self, Question};
use yaist_core::log;
use yaist_core::{command, ActionSpec, Context, Registry, Risk};

use crate::state::{Asked, Entry, LlmStatus, ModuleView, Panel, Plan, PlanStatus, State, UpdateStatus, LOG_LINES};

/// Where the output of changes is kept, under $XDG_STATE_HOME (or
/// ~/.local/state): one file per change, the newest [`LOGS_KEPT`].
const LOG_DIR: &str = "yaist/logs";
const LOGS_KEPT: usize = 100;

/// A new log file for a change, readable only by the user; older ones beyond
/// [`LOGS_KEPT`] are removed.
fn open_log(spec: &ActionSpec, command: &str) -> Option<(String, std::fs::File)> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|d| !d.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".local/state")))?;
    let dir = base.join(LOG_DIR);
    std::fs::create_dir_all(&dir).ok()?;
    let mut old: Vec<_> = std::fs::read_dir(&dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "log")).collect();
    if old.len() >= LOGS_KEPT {
        // The names start with the time, so they sort oldest first.
        old.sort();
        for p in &old[..old.len() + 1 - LOGS_KEPT] {
            let _ = std::fs::remove_file(p);
        }
    }
    let path = dir.join(format!("{}-{}.log", yaist_core::date::local_stamp(), spec.id));
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).ok()?;
    let _ = writeln!(f, "# {command}\n");
    Some((path.display().to_string(), f))
}

/// How many tool-calling rounds one request may take before we stop it.
const MAX_ROUNDS: usize = 5;

pub enum Cmd {
    /// A line from the input prompt: a `/command` or plain language.
    Submit(String),
    /// A panel button or form: run (reads) or propose (changes) an action.
    Propose {
        action: &'static str,
        raw: Map<String, Value>,
    },
    Confirm(u64),
    Cancel(u64),
    /// Propose a failed change again, with the same arguments.
    Retry(u64),
    /// Reload a module's panel.
    Refresh(usize),
    /// Count available updates again.
    CheckUpdates,
    /// The user's answer to the running change's question.
    Answer {
        id: u64,
        value: String,
    },
}

#[derive(Clone)]
struct Shared {
    reg: Arc<Registry>,
    ctx: Arc<Context>,
    state: Arc<Mutex<State>>,
    notify: Arc<dyn Fn() + Send + Sync>,
    /// Headless sessions (`yaist ask`) load no panels: a panel load can ask
    /// polkit, and nobody is looking at it.
    panels: bool,
    update_check: Arc<std::sync::atomic::AtomicBool>,
    /// Questions waiting for an answer, by id.
    questions: Arc<Mutex<HashMap<u64, oneshot::Sender<String>>>>,
    next_question: Arc<std::sync::atomic::AtomicU64>,
}

impl Shared {
    fn update<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let r = f(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()));
        (self.notify)();
        r
    }

    fn push(&self, entry: Entry) {
        self.update(|s| s.conversation.push(entry));
    }

    fn busy(&self, what: Option<String>) {
        self.update(|s| s.busy = what);
    }

    fn refresh(&self, index: usize) {
        if !self.panels {
            return;
        }
        let sh = self.clone();
        tokio::spawn(async move {
            let Some(spec) = sh.reg.modules.get(index).copied() else { return };
            sh.update(|s| s.modules[index].panel = Panel::Loading);
            let result = match sh.reg.get(spec.overview).map(|a| a.validate(&Map::new())) {
                Some(Ok(args)) => sh.reg.run(sh.ctx.clone(), spec.overview, args).await,
                Some(Err(e)) => Err(e),
                None => Err(anyhow::anyhow!("module {} has no overview action", spec.id)),
            };
            sh.update(|s| {
                s.modules[index].panel = match result {
                    Ok(out) => Panel::Ready(out),
                    Err(e) => Panel::Failed(format!("{e:#}")),
                }
            });
        });
    }

    /// Counts available updates in the background (skipped when one check
    /// is already running, and in headless sessions).
    fn check_updates(&self) {
        if !self.panels || self.update_check.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let sh = self.clone();
        tokio::spawn(async move {
            sh.update(|s| {
                if !matches!(s.updates, UpdateStatus::Known { .. }) {
                    s.updates = UpdateStatus::Checking;
                }
            });
            let status = match yaist_modules::update_counts(&sh.ctx).await {
                Ok(c) => UpdateStatus::Known { packages: c.packages, security: c.security, reboot: c.reboot, checked: Instant::now() },
                Err(e) => UpdateStatus::Failed(format!("{e:#}")),
            };
            sh.update(|s| s.updates = status);
            sh.update_check.store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }

    /// Puts questions from a running change in front of the user, one at a
    /// time, and notes each answer in the conversation.
    fn asker(&self) -> ask::Asker {
        let sh = self.clone();
        Arc::new(move |q: Question| {
            let sh = sh.clone();
            Box::pin(async move {
                let id = sh.next_question.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let (tx, rx) = oneshot::channel();
                sh.questions.lock().unwrap_or_else(|e| e.into_inner()).insert(id, tx);
                sh.update(|s| s.question = Some(Asked { id, question: q.clone() }));
                let answer = rx.await.ok();
                sh.update(|s| s.question = None);
                let chosen = answer.as_ref().and_then(|a| q.choices.iter().find(|c| &c.value == a));
                if let Some(c) = chosen {
                    sh.push(Entry::Notice(format!("{}: {}", q.title, c.label)));
                }
                answer
            })
        })
    }

    /// Appends what a running change prints to its plan card, and to its
    /// log file.
    fn logger(&self, id: u64, file: Option<Arc<Mutex<std::fs::File>>>) -> log::Logger {
        let sh = self.clone();
        Arc::new(move |line: String| {
            if let Some(f) = &file {
                use std::io::Write;
                let _ = writeln!(f.lock().unwrap_or_else(|e| e.into_inner()), "{line}");
            }
            sh.update(|s| {
                if let Some(p) = s.plan_mut(id) {
                    let log = Arc::make_mut(&mut p.log);
                    if log.len() >= LOG_LINES {
                        log.drain(..log.len() + 1 - LOG_LINES);
                    }
                    log.push(line);
                }
            })
        })
    }

    fn answer(&self, id: u64, value: String) {
        if let Some(tx) = self.questions.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) {
            let _ = tx.send(value);
        }
    }

    /// Brings a module's panel forward, as after an action from the conversation.
    fn focus_module(&self, module: &str) {
        if let Some(i) = self.update(|s| {
            let i = s.module_index(module)?;
            s.selected = i;
            Some(i)
        }) {
            self.refresh(i);
        }
    }
}

pub struct Session {
    state: Arc<Mutex<State>>,
    tx: mpsc::UnboundedSender<Cmd>,
    pub registry: Arc<Registry>,
    _runtime: tokio::runtime::Runtime,
}

impl Session {
    /// Starts the backend. `notify` is called whenever the state changes, so a
    /// GUI can schedule a repaint.
    pub fn start(notify: impl Fn() + Send + Sync + 'static) -> Self {
        Self::start_with(notify, true)
    }

    /// A session without panels, for scripted use.
    pub fn start_headless() -> Self {
        Self::start_with(|| {}, false)
    }

    fn start_with(notify: impl Fn() + Send + Sync + 'static, panels: bool) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("tokio runtime");
        let reg = Arc::new(yaist_modules::registry());
        let ctx = Arc::new(Context::new());
        let llm = LlmClient::new(LlmConfig::from_env());

        let modules = reg
            .modules
            .iter()
            .map(|m| ModuleView {
                spec: *m,
                panel: if m.overview_needs_admin { Panel::Locked } else { Panel::Loading },
                actions: reg.module_actions(m.id).into_iter().filter(|a| a.id != m.overview && !m.row_actions.contains(&a.id)).collect(),
                row_actions: m.row_actions.iter().filter_map(|id| reg.get(id).copied()).collect(),
            })
            .collect();
        let distro_name = if ctx.distro.pretty.is_empty() { "this system".to_owned() } else { ctx.distro.pretty.clone() };
        let mut conversation = vec![Entry::Notice(format!("YAiST on {distro_name}. Type /help for the list of commands."))];
        if !ctx.distro.is_opensuse() {
            conversation
                .push(Entry::Error(format!("This doesn't look like openSUSE ({}); package and repository modules may not work.", ctx.distro.id)));
        }
        let state = Arc::new(Mutex::new(State {
            distro: ctx.distro.clone(),
            modules,
            selected: 0,
            conversation,
            busy: None,
            llm: LlmStatus::Checking,
            llm_url: llm.config.url.clone(),
            updates: UpdateStatus::Checking,
            question: None,
        }));

        let sh = Shared {
            reg: reg.clone(),
            ctx: ctx.clone(),
            state: state.clone(),
            notify: Arc::new(notify),
            panels,
            update_check: Default::default(),
            questions: Default::default(),
            next_question: Arc::new(1.into()),
        };
        let (tx, mut rx) = mpsc::unbounded_channel::<Cmd>();
        let (job_tx, job_rx) = mpsc::unbounded_channel::<Cmd>();

        // Panel loads run alongside whatever the agent is doing, and answers
        // reach the change that waits for them; everything else is handled
        // in order.
        let dispatch_sh = sh.clone();
        runtime.spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    Cmd::Refresh(i) => dispatch_sh.refresh(i),
                    Cmd::CheckUpdates => dispatch_sh.check_updates(),
                    Cmd::Answer { id, value } => dispatch_sh.answer(id, value),
                    other => {
                        let _ = job_tx.send(other);
                    }
                }
            }
        });

        let agent = Agent::new(&reg, &ctx.distro, llm.clone());
        let warm = agent.warmer();
        runtime.spawn(Jobs { sh: sh.clone(), agent, pending: VecDeque::new(), call_ids: HashMap::new(), next_id: 1 }.run(job_rx));
        runtime.spawn(watch_llm(sh.clone(), llm, warm));
        // Updates: now, then every 30 minutes.
        let updates_sh = sh.clone();
        runtime.spawn(async move {
            loop {
                updates_sh.check_updates();
                tokio::time::sleep(Duration::from_secs(30 * 60)).await;
            }
        });

        let _ = tx.send(Cmd::Refresh(0));
        Self { state, tx, registry: reg, _runtime: runtime }
    }

    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    /// Selects a module, loading its panel the first time; panels that need
    /// the administrator password wait for Cmd::Refresh.
    pub fn select(&self, index: usize) {
        let load = {
            let mut s = self.state();
            if index >= s.modules.len() {
                return;
            }
            s.selected = index;
            !matches!(s.modules[index].panel, Panel::Ready(_) | Panel::Locked)
        };
        if load {
            self.send(Cmd::Refresh(index));
        }
    }
}

async fn watch_llm(sh: Shared, llm: LlmClient, warm: impl std::future::Future<Output = anyhow::Result<()>> + Send + 'static) {
    let mut warm = Some(warm);
    loop {
        let status = match llm.model_name().await {
            Ok(name) => LlmStatus::Ready(name),
            Err(_) => LlmStatus::Offline,
        };
        let ready = matches!(status, LlmStatus::Ready(_));
        let changed = sh.state.lock().map(|s| s.llm != status).unwrap_or(true);
        if ready {
            if let Some(w) = warm.take() {
                // Warm the prompt cache before reporting ready, so the first
                // request is as fast as the rest.
                sh.update(|s| s.llm = LlmStatus::Checking);
                let _ = w.await;
            }
        }
        if changed || ready {
            sh.update(|s| s.llm = status);
        }
        tokio::time::sleep(Duration::from_secs(if ready { 15 } else { 3 })).await;
    }
}

struct Jobs {
    sh: Shared,
    agent: Agent,
    /// Plans waiting for the user, oldest first.
    pending: VecDeque<u64>,
    /// Plan → the agent's tool call it answers.
    call_ids: HashMap<u64, String>,
    next_id: u64,
}

impl Jobs {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Cmd>) {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                Cmd::Submit(text) => self.submit(text.trim()).await,
                Cmd::Propose { action, raw } => {
                    self.cancel_all();
                    match self.sh.reg.get(action).copied() {
                        Some(spec) => {
                            if let Ok(args) = spec.validate(&raw) {
                                self.sh.push(Entry::Command(command::format(&spec, &args)));
                            }
                            self.invoke(spec, raw, None).await;
                        }
                        None => self.sh.push(Entry::Error(format!("unknown action {action}"))),
                    }
                }
                Cmd::Confirm(id) => self.execute(id).await,
                Cmd::Cancel(id) => self.cancel(id),
                Cmd::Retry(id) => self.retry(id),
                Cmd::Refresh(i) => self.sh.refresh(i),
                Cmd::CheckUpdates => self.sh.check_updates(),
                Cmd::Answer { id, value } => self.sh.answer(id, value),
            }
        }
    }

    async fn submit(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.cancel_all();
        if text == "/clear" {
            self.agent.clear();
            self.sh.update(|s| s.conversation.clear());
            return;
        }
        // Familiar tool commands (zypper in fish, systemctl restart sshd) are
        // commands too, with or without the slash.
        if text.starts_with('/') || command::is_tool_command(text) {
            self.sh.push(Entry::Command(text.to_owned()));
            match command::parse(&self.sh.reg, text) {
                Ok(command::Parsed::Help(module)) => self.sh.push(Entry::Notice(command::help(&self.sh.reg, module.as_deref()))),
                Ok(command::Parsed::Action { spec, raw }) => {
                    self.invoke(spec, raw, None).await;
                }
                Err(e) => self.sh.push(Entry::Error(e.to_string())),
            }
            return;
        }
        self.sh.push(Entry::User(text.to_owned()));
        self.agent_turn(text).await;
    }

    /// Runs a read action now, or turns a change into a plan card. Returns the
    /// tool result for the agent when there is one already.
    async fn invoke(&mut self, spec: ActionSpec, raw: Map<String, Value>, call_id: Option<String>) -> Option<String> {
        let args = match spec.validate(&raw) {
            Ok(a) => a,
            Err(e) => {
                if call_id.is_some() {
                    return Some(format!("Error: {e}"));
                }
                self.sh.push(Entry::Error(e.to_string()));
                return None;
            }
        };
        let cmdline = command::format(&spec, &args);
        let shell = self.sh.reg.shell_for(spec.id, &args, &self.sh.ctx.distro);
        if spec.risk == Risk::Read {
            self.sh.busy(Some(format!("Running {}…", spec.title)));
            let result = self.sh.reg.run(self.sh.ctx.clone(), spec.id, args).await;
            self.sh.busy(None);
            return match (result, call_id.is_some()) {
                (Ok(out), true) => {
                    self.sh.push(Entry::AgentRan { command: cmdline, shell, ok: true });
                    Some(out.to_llm_text(40))
                }
                (Err(e), true) => {
                    self.sh.push(Entry::AgentRan { command: cmdline, shell, ok: false });
                    Some(format!("Error: {e:#}"))
                }
                (Ok(out), false) => {
                    self.sh.push(Entry::Result { title: spec.title.to_owned(), shell, output: out });
                    None
                }
                (Err(e), false) => {
                    self.sh.push(Entry::Error(format!("{}: {e:#}", spec.title)));
                    None
                }
            };
        }
        self.plan(spec, args, cmdline, shell, call_id);
        None
    }

    /// Puts a change up for the user to confirm.
    fn plan(&mut self, spec: ActionSpec, args: yaist_core::Args, command: String, shell: Vec<String>, call_id: Option<String>) {
        let id = self.next_id;
        self.next_id += 1;
        let from_agent = call_id.is_some();
        if let Some(c) = call_id {
            self.call_ids.insert(id, c);
        }
        self.pending.push_back(id);
        self.sh.push(Entry::Plan(Plan { id, spec, args, command, shell, executed: Vec::new(), log: Arc::default(), log_file: None, status: PlanStatus::Pending, from_agent }));
        self.sh.focus_module(spec.module());
    }

    /// The same change once more, as a new card to confirm. The arguments are
    /// the validated ones of the failed plan (secrets included, which a form
    /// would show masked).
    fn retry(&mut self, id: u64) {
        let Some(p) = self.sh.update(|s| s.plan_mut(id).filter(|p| matches!(p.status, PlanStatus::Failed(_))).cloned()) else { return };
        self.cancel_all();
        self.sh.push(Entry::Command(p.command.clone()));
        self.plan(p.spec, p.args, p.command, p.shell, None);
    }

    async fn agent_turn(&mut self, text: &str) {
        let started = Instant::now();
        self.agent.push_user(text);
        self.sh.busy(Some("Thinking…".into()));
        for _ in 0..MAX_ROUNDS {
            let reply = match self.agent.step().await {
                Ok(r) => r,
                Err(e) => {
                    self.sh.push(Entry::Error(format!(
                        "The local model is not reachable ({e:#}). Start it with {}; /commands and panels work without it.",
                        crate::llm_start_hint()
                    )));
                    break;
                }
            };
            if !reply.content.is_empty() {
                self.sh.push(Entry::Assistant { text: reply.content.clone(), elapsed: started.elapsed() });
            }
            if reply.tool_calls.is_empty() {
                break;
            }
            let mut waiting = false;
            for call in reply.tool_calls {
                let Some(spec) = self.sh.reg.get(&action_id(&call.name)).copied() else {
                    self.agent.push_tool_result(&call.id, &format!("Error: there is no tool called {}", call.name));
                    continue;
                };
                let raw = match call.arguments {
                    Ok(m) => m,
                    Err(_) => {
                        self.agent.push_tool_result(&call.id, "Error: the arguments were not valid JSON");
                        continue;
                    }
                };
                match self.invoke(spec, raw, Some(call.id.clone())).await {
                    Some(result) => self.agent.push_tool_result(&call.id, &result),
                    None => waiting = true,
                }
            }
            // A plan card is up: the rest of the turn is the user's decision.
            if waiting {
                break;
            }
            self.sh.busy(Some("Thinking…".into()));
        }
        self.sh.busy(None);
    }

    async fn execute(&mut self, id: u64) {
        let Some((spec, args, command)) = self.sh.update(|s| {
            let p = s.plan_mut(id).filter(|p| p.status == PlanStatus::Pending)?;
            p.status = PlanStatus::Running;
            Some((p.spec, p.args.clone(), p.command.clone()))
        }) else {
            return;
        };
        self.pending.retain(|&p| p != id);
        self.sh.busy(Some(format!("{}…", spec.title)));
        let file = if self.sh.panels { open_log(&spec, &command) } else { None };
        let (path, file) = file.map(|(p, f)| (Some(p), Some(Arc::new(Mutex::new(f))))).unwrap_or_default();
        self.sh.update(|s| {
            if let Some(p) = s.plan_mut(id) {
                p.log_file = path;
            }
        });
        let run = self.sh.reg.run_logged(self.sh.ctx.clone(), spec.id, args);
        // Headless sessions have nobody to ask; the change takes its defaults.
        let (result, executed) =
            if self.sh.panels { log::logging(self.sh.logger(id, file.clone()), ask::asking(self.sh.asker(), run)).await } else { run.await };
        if let Some(f) = &file {
            use std::io::Write;
            let mut f = f.lock().unwrap_or_else(|e| e.into_inner());
            let _ = writeln!(f, "\n# YAiST ran:");
            for c in &executed {
                let _ = writeln!(f, "#   {c}");
            }
            let _ = match &result {
                Ok(out) => writeln!(f, "# done: {}", out.headline()),
                Err(e) => writeln!(f, "# failed: {e:#}"),
            };
        }
        self.sh.busy(None);
        let (status, note) = match result {
            Ok(out) => {
                let line = out.headline();
                (PlanStatus::Done(line.clone()), format!("Done: {line}"))
            }
            Err(e) => (PlanStatus::Failed(format!("{e:#}")), format!("Failed: {e:#}")),
        };
        self.sh.update(|s| {
            if let Some(p) = s.plan_mut(id) {
                p.status = status;
                p.executed = executed;
            }
        });
        if let Some(call) = self.call_ids.remove(&id) {
            self.agent.push_tool_result(&call, &note);
        }
        self.sh.focus_module(spec.module());
        if matches!(spec.module(), "packages" | "repos" | "upgrade" | "snapshots") {
            self.sh.check_updates();
        }
    }

    fn cancel(&mut self, id: u64) {
        let was_pending = self.sh.update(|s| match s.plan_mut(id) {
            Some(p) if p.status == PlanStatus::Pending => {
                p.status = PlanStatus::Cancelled;
                true
            }
            _ => false,
        });
        if was_pending {
            self.pending.retain(|&p| p != id);
            if let Some(call) = self.call_ids.remove(&id) {
                self.agent.push_tool_result(&call, "The user cancelled this action. Do not retry it unless asked.");
            }
        }
    }

    /// A new request supersedes plans still waiting for an answer.
    fn cancel_all(&mut self) {
        for id in std::mem::take(&mut self.pending) {
            self.cancel(id);
        }
    }
}
