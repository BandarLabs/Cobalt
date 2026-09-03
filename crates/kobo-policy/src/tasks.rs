//! Running application work off the event loop.
//!
//! An application never spawns its own thread. Threads it owned could outlive
//! the screen, hold the radio open after the reader walked away, or keep the
//! device from suspending, and none of that is visible from the outside. Here
//! every unit of work is registered, counted, cancellable, and reports back
//! exactly once.

use kobo_net::{LineStreamAction, LineStreams, RequestOptions};
use kobo_protocol::{
    Credential, CredentialUse, SecretHeader, Task, TaskError, TaskId, TaskOutcome, MAX_TASK_BYTES,
    MAX_TASK_BYTES_U32,
};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::Capability;

/// The ceiling on tasks in flight for one application at once.
///
/// Each task is a real thread and a real connection or file handle, and an
/// unbounded queue is an unbounded amount of radio time.
pub const MAX_TASKS_IN_FLIGHT: usize = 4;

/// The longest any single task may run before it is abandoned.
///
/// A task with no deadline is a task that can hold the radio open until the
/// battery is flat.
pub const TASK_DEADLINE: Duration = Duration::from_secs(30);

/// The longest a sleep task may ask for.
pub const MAX_SLEEP_SECONDS: u32 = 300;

/// Why a task could not even be started.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RejectReason {
    /// Too many tasks are already running.
    AtCapacity,
    /// That identifier is already in use by a task still running.
    DuplicateId,
}

/// One finished task, ready to be reported to the application.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Finished {
    pub task: TaskId,
    pub outcome: TaskOutcome,
}

struct Running {
    cancel: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    stream_url: Option<String>,
}

#[derive(Clone, Debug)]
struct SecretStore {
    root: PathBuf,
    app: Option<String>,
}

/// The host-provided network implementation a task's fetch runs through.
///
/// It is a named type because the runtime, its builder and the worker all
/// mention it, and spelling the whole signature in three places invites them
/// to drift apart. The fourth argument is the resolved runtime credential,
/// kept separate so the backend retains its provenance; the fifth is the
/// application's non-secret headers, already checked against the ones the
/// runtime owns. These are the same guarantees `Poster` makes for `Post`.
pub type Fetcher = dyn Fn(
        &str,
        u32,
        u32,
        Option<(&str, &str)>,
        &[(&str, &str)],
        RequestOptions,
        &AtomicBool,
    ) -> Result<Vec<u8>, TaskError>
    + Send
    + Sync;

/// The host-provided implementation a `Post` task runs through.
///
/// The credential is the fourth argument, as a header name and its complete
/// value, and arrives already resolved: the application named a secret and
/// never supplied or observed it. The fifth argument is the non-secret headers
/// the application asked for, already checked against the ones the runtime
/// owns.
pub type Poster = dyn Fn(
        &str,
        &[u8],
        &str,
        Option<(&str, &str)>,
        &[(&str, &str)],
        u32,
        RequestOptions,
        &AtomicBool,
    ) -> Result<Vec<u8>, TaskError>
    + Send
    + Sync;

/// Decides whether one named credential may be sent to one URL.
///
/// Secret files alone are not authority: without this second decision an
/// application could name a real key and an attacker-controlled destination.
pub type CredentialAuthorizer =
    dyn Fn(&Credential, &str, CredentialUse, Option<&str>, Option<&str>) -> bool + Send + Sync;

/// Headers an application may not set, because the runtime decides them.
///
/// `Authorization` is here even though a credential may legitimately go there:
/// the runtime puts it on from a named secret, so an application setting it
/// directly is either supplying its own key (which defeats the whole point of
/// never letting an application hold one) or overwriting the runtime's.
const RESERVED_HEADERS: &[&str] = &[
    "authorization",
    "host",
    "content-length",
    "content-type",
    "connection",
    "transfer-encoding",
    "range",
    "accept-encoding",
];

const LINE_STREAM_HEADER: &str = "x-cobalt-line-stream";
const WAIT_HEADER: &str = "x-cobalt-wait-until-cancelled";
const RATE_LIMIT_HEADER: &str = "x-cobalt-rate-limit";

/// Whether an application may set this header itself.
#[must_use]
pub fn header_is_the_applications_to_set(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    !name.starts_with("x-cobalt-") && !RESERVED_HEADERS.contains(&name.as_str())
}

/// Turns a named credential into the header it will be sent as.
///
/// The one place that has both the naming convention and the value, so nothing
/// downstream has to know that Bearer is spelled differently from an API key
/// or that Basic is encoded at all. Shared by fetch and post: a catalogue
/// behind a subscription is reached with a GET, and a credential that only
/// worked on a POST would mean recognising a gated feed and never opening it.
fn resolved_credential(
    wanted: Option<&kobo_protocol::Credential>,
    url: &str,
    usage: CredentialUse,
    body: Option<&str>,
    content_type: Option<&str>,
    credentials: Option<&CredentialAuthorizer>,
    secrets: Option<&SecretStore>,
) -> Result<Option<(String, String)>, TaskError> {
    let Some(wanted) = wanted else {
        return Ok(None);
    };
    if credentials.is_none_or(|allows| !allows(wanted, url, usage, body, content_type)) {
        return Err(TaskError::Denied);
    }
    // Not `Denied`. The application asked for a key it is allowed to ask for,
    // by the name the runtime publishes, and the check above already said so.
    // What is missing is the key itself, which is the reader owner's to
    // install.
    let Some(value) = scoped_secret(secrets, &wanted.secret) else {
        return Err(TaskError::NoCredential);
    };
    Ok(Some((
        wanted.header_name().to_owned(),
        match wanted.header {
            SecretHeader::Bearer => format!("Bearer {value}"),
            SecretHeader::Named(_) => value,
            // Encoded here rather than by the application, which is the whole
            // point: an application asking for a gated feed never holds the
            // address or the password that make up the pair.
            SecretHeader::Basic => format!("Basic {}", base64(value.as_bytes())),
        },
    )))
}

fn credential_is_authorized(
    wanted: Option<&Credential>,
    url: &str,
    usage: CredentialUse,
    body: Option<&str>,
    content_type: Option<&str>,
    credentials: Option<&CredentialAuthorizer>,
) -> Result<(), TaskError> {
    let Some(wanted) = wanted else {
        return Err(TaskError::Denied);
    };
    if credentials.is_some_and(|allows| allows(wanted, url, usage, body, content_type)) {
        Ok(())
    } else {
        Err(TaskError::Denied)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Controls {
    stream: Option<LineStreamAction>,
    wait_until_cancelled: bool,
    report_rate_limit: bool,
}

struct PreparedHeaders<'a> {
    forwarded: Vec<(&'a str, &'a str)>,
    controls: Controls,
}

/// Checks a task's headers, separates runtime controls, and hands only ordinary
/// non-secret pairs to the network backend.
///
/// Shared by `Fetch` and `Post` rather than duplicated, because the two
/// checks have to keep agreeing: `Range` is reserved for exactly the reason
/// `Authorization` is, and a fork here is how one of them would quietly stop
/// enforcing it.
fn own_headers(headers: &[kobo_protocol::Header]) -> Result<PreparedHeaders<'_>, TaskError> {
    let mut controls = Controls::default();
    let mut forwarded = Vec::with_capacity(headers.len());
    for header in headers {
        let name = header.name.to_ascii_lowercase();
        match name.as_str() {
            LINE_STREAM_HEADER => {
                if controls.stream.is_some() {
                    return Err(TaskError::Denied);
                }
                controls.stream = Some(match header.value.as_str() {
                    "open" => LineStreamAction::Open,
                    "next" => LineStreamAction::Next,
                    "close" => LineStreamAction::Close,
                    _ => return Err(TaskError::Denied),
                });
            }
            WAIT_HEADER => {
                if controls.wait_until_cancelled || header.value != "1" {
                    return Err(TaskError::Denied);
                }
                controls.wait_until_cancelled = true;
            }
            RATE_LIMIT_HEADER => {
                if controls.report_rate_limit || header.value != "1" {
                    return Err(TaskError::Denied);
                }
                controls.report_rate_limit = true;
            }
            _ if name.starts_with("x-cobalt-")
                || !header_is_the_applications_to_set(&header.name) =>
            {
                return Err(TaskError::Denied);
            }
            _ => forwarded.push((header.name.as_str(), header.value.as_str())),
        }
    }
    Ok(PreparedHeaders {
        forwarded,
        controls,
    })
}

fn line_stream_url(work: &Task) -> Option<String> {
    let Task::Fetch { url, headers, .. } = work else {
        return None;
    };
    headers
        .iter()
        .any(|header| {
            header.name.eq_ignore_ascii_case(LINE_STREAM_HEADER)
                && matches!(header.value.as_str(), "open" | "next")
        })
        .then(|| url.clone())
}

/// Told, from the finishing task's own thread, that a result is now waiting.
///
/// It carries nothing. The runtime still calls [`TaskRunner::drain`] for the
/// outcome, so this can never become a second, divergent delivery path.
pub type Wake = dyn Fn() + Send + Sync;

/// The longest a stored secret may be.
///
/// Every credential these applications use is far shorter. A ceiling means a
/// file that is not a secret at all (a log somebody dropped in the directory,
/// a truncated download) is refused rather than sent to a server.
pub const MAX_SECRET_BYTES: usize = 4096;

/// Runs application tasks under policy.
pub struct TaskRunner {
    /// The directory a `ReadFile` task is confined to. Every path is resolved
    /// against this and anything that escapes it is refused.
    root: PathBuf,
    granted: Vec<Capability>,
    running: HashMap<TaskId, Running>,
    sender: Sender<Finished>,
    receiver: Receiver<Finished>,
    fetch: Option<Arc<Fetcher>>,
    post: Option<Arc<Poster>>,
    line_streams: Option<Arc<LineStreams>>,
    /// Where named secrets are read from, if anywhere.
    secrets: Option<SecretStore>,
    /// Which secret and destination pairs this application is trusted to use.
    credentials: Option<Arc<CredentialAuthorizer>>,
    /// Called once, from the task's own thread, the moment a result is ready.
    ///
    /// A runtime that only looks for results when something else happens keeps
    /// an answer sitting in the channel until the next touch, which reads as a
    /// hung application. This exists so the runtime can be woken instead of
    /// polled: a device that idles at zero power must not spin a loop to
    /// discover work it could have been told about.
    wake: Option<Arc<Wake>>,
}

impl std::fmt::Debug for TaskRunner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TaskRunner")
            .field("root", &self.root)
            .field("granted", &self.granted)
            .field("running", &self.running.len())
            .field("networked", &self.fetch.is_some())
            // Deliberately whether, not where and never what. This type ends
            // up in error messages and traces.
            .field("posts", &self.post.is_some())
            .field("line_streams", &self.line_streams.is_some())
            .field("secrets", &self.secrets.is_some())
            .field("credential_policy", &self.credentials.is_some())
            .finish_non_exhaustive()
    }
}

impl TaskRunner {
    /// A runner with no network at all.
    ///
    /// This is what the simulator uses. A fetch is refused rather than faked,
    /// because an application that only ever sees invented responses is an
    /// application whose error handling has never run.
    #[must_use]
    pub fn simulated(root: impl Into<PathBuf>) -> Self {
        let (sender, receiver) = channel();
        Self {
            root: root.into(),
            granted: Vec::new(),
            running: HashMap::new(),
            sender,
            receiver,
            fetch: None,
            post: None,
            line_streams: None,
            secrets: None,
            credentials: None,
            wake: None,
        }
    }

    /// Grants capabilities. Anything not granted is refused at submission.
    #[must_use]
    pub fn with_capabilities(mut self, granted: impl IntoIterator<Item = Capability>) -> Self {
        self.granted = granted.into_iter().collect();
        self
    }

    /// Supplies the network backend used by `Fetch`.
    #[must_use]
    pub fn with_fetch(mut self, fetch: Arc<Fetcher>) -> Self {
        self.fetch = Some(fetch);
        self
    }

    /// Supplies the network backend used by `Post`.
    #[must_use]
    pub fn with_post(mut self, post: Arc<Poster>) -> Self {
        self.post = Some(post);
        self
    }

    /// Supplies the per-application retained line-stream backend.
    #[must_use]
    pub fn with_line_streams(mut self, streams: Arc<LineStreams>) -> Self {
        self.line_streams = Some(streams);
        self
    }

    /// Supplies the directory named secrets are read from.
    ///
    /// A runner without one refuses every task that names a secret, which is
    /// why the simulator can run a networked application without ever holding
    /// a credential.
    #[must_use]
    pub fn with_secrets(mut self, directory: impl Into<PathBuf>) -> Self {
        self.secrets = Some(SecretStore {
            root: directory.into(),
            app: None,
        });
        self
    }

    /// Supplies app-scoped secrets plus the owner-managed global fallback.
    ///
    /// Lookup first tries `apps/{verified-app}/{name}`, then the legacy/global
    /// `{name}` file. This preserves CLI-installed owner credentials while an
    /// app-entered value can affect only the verified caller.
    #[must_use]
    pub fn with_app_secrets(
        mut self,
        directory: impl Into<PathBuf>,
        verified_app: impl Into<String>,
    ) -> Self {
        self.secrets = Some(SecretStore {
            root: directory.into(),
            app: Some(verified_app.into()),
        });
        self
    }

    /// Supplies the application-specific credential destination policy.
    ///
    /// A runner without one may still perform unauthenticated requests, but
    /// refuses every request naming a secret.
    #[must_use]
    pub fn with_credential_policy(mut self, policy: Arc<CredentialAuthorizer>) -> Self {
        self.credentials = Some(policy);
        self
    }

    /// Supplies the callback that tells the runtime a result is ready.
    ///
    /// Without one the runtime learns about a finished task only when it next
    /// looks, which is what made a reply that had already arrived appear not to
    /// have arrived at all.
    #[must_use]
    pub fn with_wake(mut self, wake: Arc<Wake>) -> Self {
        self.wake = Some(wake);
        self
    }

    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.running.len()
    }

    fn grants(&self, capability: Capability) -> bool {
        self.granted.contains(&capability)
    }

    /// Starts a task, or refuses it.
    ///
    /// A refusal that the application caused, such as asking for a capability
    /// it does not hold, is delivered as a normal outcome rather than an error,
    /// so an application always hears back about work it submitted.
    ///
    /// # Errors
    ///
    /// Returns the reason when the task could not be admitted at all.
    pub fn submit(&mut self, task: TaskId, work: Task) -> Result<(), RejectReason> {
        if self.running.contains_key(&task) {
            return Err(RejectReason::DuplicateId);
        }
        if self.running.len() >= MAX_TASKS_IN_FLIGHT {
            return Err(RejectReason::AtCapacity);
        }

        let required = match &work {
            Task::Fetch { .. } | Task::Post { .. } => Some(Capability::Network),
            Task::ReadFile { .. } | Task::Sleep { .. } => None,
        };
        if let Some(capability) = required {
            if !self.grants(capability) {
                let _ = self.sender.send(Finished {
                    task,
                    outcome: TaskOutcome::Failed(TaskError::Denied),
                });
                if let Some(wake) = &self.wake {
                    wake();
                }
                return Ok(());
            }
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let stream_url = line_stream_url(&work);
        let sender = self.sender.clone();
        let root = self.root.clone();
        let fetch = self.fetch.clone();
        let post = self.post.clone();
        let line_streams = self.line_streams.clone();
        let secrets = self.secrets.clone();
        let credentials = self.credentials.clone();
        let flag = Arc::clone(&cancel);
        let wake = self.wake.clone();
        let handle = thread::Builder::new()
            .name(format!("kobo-task-{}", task.0))
            .spawn(move || {
                let outcome = run(
                    &work,
                    &root,
                    Backends {
                        fetch: fetch.as_deref(),
                        post: post.as_deref(),
                        line_streams: line_streams.as_deref(),
                        secrets: secrets.as_ref(),
                        credentials: credentials.as_deref(),
                    },
                    &flag,
                );
                let outcome = if flag.load(Ordering::SeqCst) {
                    TaskOutcome::Cancelled
                } else {
                    outcome
                };
                let _ = sender.send(Finished { task, outcome });
                // After the result is in the channel, never before, so the
                // woken runtime always finds something to drain.
                if let Some(wake) = wake {
                    wake();
                }
            })
            .map_err(|_| RejectReason::AtCapacity)?;
        self.running.insert(
            task,
            Running {
                cancel,
                handle: Some(handle),
                stream_url,
            },
        );
        Ok(())
    }

    /// Asks a task to stop.
    ///
    /// The task still reports back, so an application never has to guess
    /// whether a cancellation took effect.
    pub fn cancel(&mut self, task: TaskId) {
        if let Some(running) = self.running.get(&task) {
            running.cancel.store(true, Ordering::SeqCst);
            if let (Some(streams), Some(url)) =
                (self.line_streams.as_deref(), running.stream_url.as_deref())
            {
                streams.close(url);
            }
        }
    }

    /// Collects any tasks that have finished, without blocking.
    pub fn drain(&mut self) -> Vec<Finished> {
        let mut finished = Vec::new();
        while let Ok(item) = self.receiver.try_recv() {
            self.reap(item.task);
            finished.push(item);
        }
        finished
    }

    /// Waits up to `timeout` for one task to finish.
    #[must_use]
    pub fn wait(&mut self, timeout: Duration) -> Option<Finished> {
        match self.receiver.recv_timeout(timeout) {
            Ok(item) => {
                self.reap(item.task);
                Some(item)
            }
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => None,
        }
    }

    fn reap(&mut self, task: TaskId) {
        if let Some(mut running) = self.running.remove(&task) {
            if let Some(handle) = running.handle.take() {
                let _ = handle.join();
            }
        }
    }

    /// Cancels everything and waits for it to stop.
    ///
    /// Called when an application exits. Without this a task outliving its
    /// application would keep the radio or the file handle it was using, with
    /// nothing left to report to.
    pub fn shutdown(&mut self) {
        for running in self.running.values() {
            running.cancel.store(true, Ordering::SeqCst);
        }
        let tasks = self.running.keys().copied().collect::<Vec<_>>();
        for task in tasks {
            self.reap(task);
        }
        while self.receiver.try_recv().is_ok() {}
        if let Some(streams) = &self.line_streams {
            streams.close_all();
        }
    }
}

impl Drop for TaskRunner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// What the host lets a task reach.
#[derive(Clone, Copy)]
struct Backends<'a> {
    fetch: Option<&'a Fetcher>,
    post: Option<&'a Poster>,
    line_streams: Option<&'a LineStreams>,
    secrets: Option<&'a SecretStore>,
    credentials: Option<&'a CredentialAuthorizer>,
}

/// Reads a named secret.
///
/// The name is treated as one path component and nothing else: no separators,
/// no dots, nothing that could walk out of the directory. An application
/// choosing its own secret name must not be able to turn that into a read of
/// an arbitrary file, because the value goes straight into a request to a
/// server the application also chose.
fn scoped_secret(store: Option<&SecretStore>, name: &str) -> Option<String> {
    let store = store?;
    if !crate::credentials::valid_secret_name(name) {
        return None;
    }
    if let Some(app) = store.app.as_deref() {
        let path = crate::credentials::app_secret_path(&store.root, app, name)?;
        if let Some(value) = read_secret(&store.root, &path) {
            return Some(value);
        }
    }
    read_secret(&store.root, &store.root.join(name))
}

#[cfg(test)]
fn secret(directory: Option<&Path>, name: &str) -> Option<String> {
    let directory = directory?;
    if !crate::credentials::valid_secret_name(name) {
        return None;
    }
    read_secret(directory, &directory.join(name))
}

fn read_secret(root: &Path, path: &Path) -> Option<String> {
    let root_metadata = std::fs::symlink_metadata(root).ok()?;
    if !root_metadata.file_type().is_dir() {
        return None;
    }
    let relative = path.strip_prefix(root).ok()?;
    let mut checked = root.to_owned();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return None;
        };
        checked.push(component);
        let metadata = std::fs::symlink_metadata(&checked).ok()?;
        if checked == path {
            if !metadata.file_type().is_file() {
                return None;
            }
        } else if !metadata.file_type().is_dir() {
            return None;
        }
    }
    let file = kobo_abi::open_read_nofollow(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_SECRET_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_SECRET_BYTES {
        return None;
    }
    let value = String::from_utf8(bytes).ok()?;
    let value = value.trim().to_owned();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn run(work: &Task, root: &Path, backends: Backends<'_>, cancel: &AtomicBool) -> TaskOutcome {
    match work {
        Task::Sleep { seconds } => {
            // Polled in short slices rather than slept in one call, so a
            // cancelled five minute sleep stops now instead of in five minutes.
            let total = Duration::from_secs(u64::from((*seconds).min(MAX_SLEEP_SECONDS)));
            let step = Duration::from_millis(50);
            let mut waited = Duration::ZERO;
            while waited < total {
                if cancel.load(Ordering::SeqCst) {
                    return TaskOutcome::Cancelled;
                }
                thread::sleep(step.min(total.saturating_sub(waited)));
                waited += step;
            }
            TaskOutcome::Completed(Vec::new())
        }
        Task::ReadFile { path } => match resolve(root, path) {
            None => TaskOutcome::Failed(TaskError::Denied),
            Some(path) => match std::fs::File::open(&path) {
                Err(_) => TaskOutcome::Failed(TaskError::NotFound),
                Ok(file) => {
                    let mut bytes = Vec::new();
                    // Bounded by one more byte than the ceiling, so a file
                    // exactly at the limit is accepted and one over is
                    // reported rather than silently truncated.
                    match file.take(MAX_TASK_BYTES as u64 + 1).read_to_end(&mut bytes) {
                        Err(_) => TaskOutcome::Failed(TaskError::NotFound),
                        Ok(_) if bytes.len() > MAX_TASK_BYTES => {
                            TaskOutcome::Failed(TaskError::TooLarge)
                        }
                        Ok(_) => TaskOutcome::Completed(bytes),
                    }
                }
            },
        },
        Task::Fetch {
            url,
            offset,
            max_bytes,
            credential: wanted,
            headers,
        } => run_fetch(
            url,
            *offset,
            *max_bytes,
            wanted.as_ref(),
            headers,
            backends,
            cancel,
        ),
        Task::Post {
            url,
            body,
            content_type,
            credential: wanted,
            headers,
            max_bytes,
        } => run_post(
            url,
            body,
            content_type,
            *max_bytes,
            wanted.as_ref(),
            headers,
            backends,
            cancel,
        ),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "ordinary fetch and retained-stream fetch share one credential and header gate"
)]
fn run_fetch(
    url: &str,
    offset: u32,
    max_bytes: u32,
    wanted: Option<&Credential>,
    headers: &[kobo_protocol::Header],
    backends: Backends<'_>,
    cancel: &AtomicBool,
) -> TaskOutcome {
    // `Range` and the runtime-owned credential cannot be replaced by app
    // headers; `own_headers` applies the same gate used for POST.
    let prepared = match own_headers(headers) {
        Ok(prepared) => prepared,
        Err(error) => return TaskOutcome::Failed(error),
    };
    let (extra, controls) = (prepared.forwarded, prepared.controls);
    if controls.wait_until_cancelled {
        return TaskOutcome::Failed(TaskError::Denied);
    }
    if let Some(action) = controls.stream {
        if offset != 0 || wanted.is_none() {
            return TaskOutcome::Failed(TaskError::Denied);
        }
        let Some(streams) = backends.line_streams else {
            return TaskOutcome::Failed(TaskError::Denied);
        };
        if action == LineStreamAction::Close {
            let authorized = credential_is_authorized(
                wanted,
                url,
                CredentialUse::Fetch,
                None,
                None,
                backends.credentials,
            );
            return match authorized {
                Ok(()) => match streams.request(
                    action,
                    url,
                    max_bytes.min(MAX_TASK_BYTES_U32),
                    None,
                    &extra,
                    RequestOptions {
                        report_rate_limit: controls.report_rate_limit,
                        wait_until_cancelled: false,
                    },
                    cancel,
                ) {
                    Ok(bytes) => TaskOutcome::Completed(bytes),
                    Err(error) => TaskOutcome::Failed(error),
                },
                Err(error) => TaskOutcome::Failed(error),
            };
        }
        let resolved = match resolved_credential(
            wanted,
            url,
            CredentialUse::Fetch,
            None,
            None,
            backends.credentials,
            backends.secrets,
        ) {
            Ok(credential) => credential,
            Err(error) => {
                streams.close(url);
                return TaskOutcome::Failed(error);
            }
        };
        return match streams.request(
            action,
            url,
            max_bytes.min(MAX_TASK_BYTES_U32),
            resolved
                .as_ref()
                .map(|(name, value)| (name.as_str(), value.as_str())),
            &extra,
            RequestOptions {
                report_rate_limit: controls.report_rate_limit,
                wait_until_cancelled: false,
            },
            cancel,
        ) {
            Ok(bytes) => TaskOutcome::Completed(bytes),
            Err(error) => TaskOutcome::Failed(error),
        };
    }
    let Some(fetch) = backends.fetch else {
        return TaskOutcome::Failed(TaskError::Denied);
    };
    let resolved = match resolved_credential(
        wanted,
        url,
        CredentialUse::Fetch,
        None,
        None,
        backends.credentials,
        backends.secrets,
    ) {
        Ok(credential) => credential,
        Err(error) => return TaskOutcome::Failed(error),
    };
    match fetch(
        url,
        offset,
        max_bytes.min(MAX_TASK_BYTES_U32),
        resolved
            .as_ref()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        &extra,
        RequestOptions {
            report_rate_limit: controls.report_rate_limit,
            wait_until_cancelled: false,
        },
        cancel,
    ) {
        Ok(bytes) => TaskOutcome::Completed(bytes),
        Err(error) => TaskOutcome::Failed(error),
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the closed Task::Post fields and runtime backends are passed explicitly"
)]
fn run_post(
    url: &str,
    body: &str,
    content_type: &str,
    max_bytes: u32,
    wanted: Option<&Credential>,
    headers: &[kobo_protocol::Header],
    backends: Backends<'_>,
    cancel: &AtomicBool,
) -> TaskOutcome {
    let Some(post) = backends.post else {
        return TaskOutcome::Failed(TaskError::Denied);
    };
    let prepared = match own_headers(headers) {
        Ok(prepared) => prepared,
        Err(error) => return TaskOutcome::Failed(error),
    };
    let (extra, controls) = (prepared.forwarded, prepared.controls);
    if controls.stream.is_some() || (controls.wait_until_cancelled && wanted.is_none()) {
        return TaskOutcome::Failed(TaskError::Denied);
    }
    let resolved = match resolved_credential(
        wanted,
        url,
        CredentialUse::Post,
        Some(body),
        Some(content_type),
        backends.credentials,
        backends.secrets,
    ) {
        Ok(credential) => credential,
        Err(error) => return TaskOutcome::Failed(error),
    };
    match post(
        url,
        body.as_bytes(),
        content_type,
        resolved
            .as_ref()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        &extra,
        max_bytes.min(MAX_TASK_BYTES_U32),
        RequestOptions {
            report_rate_limit: controls.report_rate_limit,
            wait_until_cancelled: controls.wait_until_cancelled,
        },
        cancel,
    ) {
        Ok(bytes) => TaskOutcome::Completed(bytes),
        Err(error) => TaskOutcome::Failed(error),
    }
}

/// Resolves a task path inside the sandbox root, or refuses it.
///
/// Rejects absolute paths and any parent component outright rather than
/// canonicalising and comparing. Canonicalising is the usual approach and it is
/// wrong here: the user partition is vfat and case insensitive, so a string
/// comparison after canonicalisation can be defeated by changing case, and a
/// path that does not exist yet cannot be canonicalised at all.
fn resolve(root: &Path, path: &str) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return None;
    }
    let mut resolved = root.to_path_buf();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => resolved.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(resolved)
}

/// Standard base64, which is how a Basic credential is spelled on the wire.
///
/// Written out rather than taken from a crate: it is twenty lines, it runs on
/// a credential a few dozen bytes long, and a dependency that can see secrets
/// is a dependency worth not having.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut block = [0u8; 3];
        block[..chunk.len()].copy_from_slice(chunk);
        let packed = (u32::from(block[0]) << 16) | (u32::from(block[1]) << 8) | u32::from(block[2]);
        for index in 0..4 {
            // Every byte past the ones that were there is padding, and the
            // count of them is what says how much of the last block is real.
            if index <= chunk.len() {
                let shift = 18 - index * 6;
                let at = usize::try_from((packed >> shift) & 0b0011_1111).unwrap_or(0);
                out.push(char::from(ALPHABET[at]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_protocol::{Credential, Header};
    use std::sync::atomic::AtomicUsize;

    fn temp_root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("kobo-tasks-{name}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&path);
        path
    }

    fn collect(runner: &mut TaskRunner, expected: usize) -> Vec<Finished> {
        let mut finished = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while finished.len() < expected && std::time::Instant::now() < deadline {
            if let Some(item) = runner.wait(Duration::from_millis(100)) {
                finished.push(item);
            }
        }
        finished
    }

    #[test]
    fn a_task_needing_a_capability_the_app_lacks_is_refused_not_run() {
        let mut runner = TaskRunner::simulated(temp_root("denied"));
        runner
            .submit(
                TaskId(1),
                Task::Fetch {
                    url: "https://example.invalid/catalog".into(),
                    offset: 0,
                    max_bytes: 1024,
                    credential: None,
                    headers: Vec::new(),
                },
            )
            .expect("submitted");
        let finished = collect(&mut runner, 1);
        assert_eq!(
            finished,
            vec![Finished {
                task: TaskId(1),
                outcome: TaskOutcome::Failed(TaskError::Denied),
            }]
        );
    }

    #[test]
    fn a_refused_task_still_reports_back() {
        // An application that submitted work must always hear about it, or it
        // will sit showing an activity row forever.
        let mut runner = TaskRunner::simulated(temp_root("reports"));
        runner
            .submit(
                TaskId(7),
                Task::Fetch {
                    url: "https://example.invalid".into(),
                    offset: 0,
                    max_bytes: 16,
                    credential: None,
                    headers: Vec::new(),
                },
            )
            .expect("submitted");
        assert_eq!(collect(&mut runner, 1).len(), 1);
    }

    #[test]
    fn a_finished_task_wakes_the_runtime_rather_than_waiting_to_be_noticed() {
        // The defect this covers: the runtime drained results only when some
        // other event arrived, so a reply that had already come back sat unread
        // until the owner touched the panel.
        let woken = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&woken);
        let mut runner = TaskRunner::simulated(temp_root("wake")).with_wake(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        runner
            .submit(TaskId(3), Task::Sleep { seconds: 0 })
            .expect("submitted");
        assert_eq!(collect(&mut runner, 1).len(), 1);
        assert_eq!(woken.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_refusal_wakes_the_runtime_too() {
        // A denial is delivered without a thread, so it has its own path to the
        // channel and therefore its own way of being missed.
        let woken = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&woken);
        let mut runner =
            TaskRunner::simulated(temp_root("wake-refusal")).with_wake(Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }));
        runner
            .submit(
                TaskId(4),
                Task::Fetch {
                    url: "https://example.invalid".into(),
                    offset: 0,
                    max_bytes: 16,
                    credential: None,
                    headers: Vec::new(),
                },
            )
            .expect("submitted");
        assert_eq!(collect(&mut runner, 1).len(), 1);
        assert_eq!(woken.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn capacity_is_bounded_rather_than_queued_without_limit() {
        let mut runner = TaskRunner::simulated(temp_root("capacity"));
        for index in 0..MAX_TASKS_IN_FLIGHT {
            runner
                .submit(
                    TaskId(u32::try_from(index).expect("a small index") + 1),
                    Task::Sleep { seconds: 30 },
                )
                .expect("submitted");
        }
        assert_eq!(
            runner.submit(TaskId(99), Task::Sleep { seconds: 1 }),
            Err(RejectReason::AtCapacity)
        );
        runner.shutdown();
    }

    #[test]
    fn a_reused_identifier_is_refused_so_two_tasks_cannot_report_as_one() {
        let mut runner = TaskRunner::simulated(temp_root("duplicate"));
        runner
            .submit(TaskId(1), Task::Sleep { seconds: 30 })
            .expect("submitted");
        assert_eq!(
            runner.submit(TaskId(1), Task::Sleep { seconds: 1 }),
            Err(RejectReason::DuplicateId)
        );
        runner.shutdown();
    }

    #[test]
    fn cancelling_a_long_sleep_returns_promptly_rather_than_after_the_sleep() {
        let mut runner = TaskRunner::simulated(temp_root("cancel"));
        runner
            .submit(TaskId(1), Task::Sleep { seconds: 300 })
            .expect("submitted");
        let started = std::time::Instant::now();
        runner.cancel(TaskId(1));
        let finished = collect(&mut runner, 1);
        assert_eq!(finished[0].outcome, TaskOutcome::Cancelled);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "cancellation waited for the sleep to finish"
        );
    }

    #[test]
    fn a_file_inside_the_sandbox_is_read() {
        let root = temp_root("read");
        std::fs::write(root.join("note.txt"), b"hello").expect("write");
        let mut runner = TaskRunner::simulated(&root);
        runner
            .submit(
                TaskId(1),
                Task::ReadFile {
                    path: "note.txt".into(),
                },
            )
            .expect("submitted");
        let finished = collect(&mut runner, 1);
        assert_eq!(
            finished[0].outcome,
            TaskOutcome::Completed(b"hello".to_vec())
        );
    }

    #[test]
    fn paths_that_climb_out_of_the_sandbox_are_refused() {
        for escape in [
            "../../../etc/passwd",
            "/etc/passwd",
            "sub/../../outside",
            "..",
        ] {
            assert!(
                resolve(Path::new("/tmp/sandbox"), escape).is_none(),
                "{escape} escaped the sandbox"
            );
        }
    }

    #[test]
    fn ordinary_paths_resolve_inside_the_sandbox() {
        let resolved = resolve(Path::new("/tmp/sandbox"), "data/./file.txt").expect("resolved");
        assert_eq!(resolved, Path::new("/tmp/sandbox/data/file.txt"));
    }

    #[test]
    fn shutdown_stops_everything_so_nothing_outlives_the_application() {
        let mut runner = TaskRunner::simulated(temp_root("shutdown"));
        for index in 0..MAX_TASKS_IN_FLIGHT {
            runner
                .submit(
                    TaskId(u32::try_from(index).expect("a small index") + 1),
                    Task::Sleep { seconds: 300 },
                )
                .expect("submitted");
        }
        let started = std::time::Instant::now();
        runner.shutdown();
        assert_eq!(runner.in_flight(), 0);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn shutdown_interrupts_a_stream_blocked_writing_its_tls_request_head() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind blocking server");
        let port = listener.local_addr().expect("server address").port();
        let (report_accepted, accepted) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (_socket, _) = listener.accept().expect("accept stream");
            report_accepted.send(()).expect("report connection");
            let _ = wait.recv_timeout(Duration::from_secs(5));
        });
        let secrets = secret_dir("shutdown-stream-head");
        let streams = Arc::new(LineStreams::default());
        let mut runner = TaskRunner::simulated(temp_root("shutdown-stream-root"))
            .with_capabilities([Capability::Network])
            .with_secrets(secrets)
            .with_line_streams(streams)
            .with_credential_policy(Arc::new(|_, _, _, _, _| true));
        runner
            .submit(
                TaskId(1),
                Task::Fetch {
                    url: format!("https://localhost:{port}/api/stream/event"),
                    offset: 0,
                    max_bytes: 4096,
                    credential: Some(Credential::bearer("openai")),
                    headers: vec![
                        Header::new("Accept", "application/x-ndjson"),
                        Header::new("X-Cobalt-Line-Stream", "open"),
                    ],
                },
            )
            .expect("submit stream");
        accepted
            .recv_timeout(Duration::from_secs(2))
            .expect("stream reached TCP server");
        let started = std::time::Instant::now();
        runner.shutdown();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(runner.in_flight(), 0);
        release.send(()).expect("release server");
        server.join().expect("blocking server");
    }

    #[test]
    fn a_granted_fetch_reaches_the_backend() {
        let mut runner = TaskRunner::simulated(temp_root("fetch"))
            .with_capabilities([Capability::Network])
            .with_fetch(Arc::new(|url: &str, _, _, _, _, _, _| {
                Ok(url.as_bytes().to_vec())
            }));
        runner
            .submit(
                TaskId(1),
                Task::Fetch {
                    url: "https://example.test/a".into(),
                    offset: 0,
                    max_bytes: 128,
                    credential: None,
                    headers: Vec::new(),
                },
            )
            .expect("submitted");
        let finished = collect(&mut runner, 1);
        assert_eq!(
            finished[0].outcome,
            TaskOutcome::Completed(b"https://example.test/a".to_vec())
        );
    }

    #[test]
    fn a_fetch_header_reaches_the_backend_the_application_asked_for_it_by() {
        let mut runner = TaskRunner::simulated(temp_root("fetch-headers"))
            .with_capabilities([Capability::Network])
            .with_fetch(Arc::new(|_, _, _, _, headers: &[(&str, &str)], _, _| {
                Ok(headers
                    .iter()
                    .map(|(name, value)| format!("{name}: {value}"))
                    .collect::<Vec<_>>()
                    .join("\n")
                    .into_bytes())
            }));
        runner
            .submit(
                TaskId(1),
                Task::Fetch {
                    url: "https://example.test/catalog".into(),
                    offset: 0,
                    max_bytes: 128,
                    credential: None,
                    headers: vec![Header::new("Accept", "application/opds+json")],
                },
            )
            .expect("submitted");
        let finished = collect(&mut runner, 1);
        assert_eq!(
            finished[0].outcome,
            TaskOutcome::Completed(b"Accept: application/opds+json".to_vec())
        );
    }

    #[test]
    fn runtime_network_controls_are_validated_and_never_forwarded_upstream() {
        let directory = secret_dir("controls");
        let mut runner = TaskRunner::simulated(temp_root("controls-root"))
            .with_capabilities([Capability::Network])
            .with_secrets(directory)
            .with_credential_policy(Arc::new(|credential, url, usage, body, content_type| {
                credential.secret == "openai"
                    && url == "https://example.invalid/seek"
                    && usage == CredentialUse::Post
                    && body == Some("fixed=1")
                    && content_type == Some("application/x-www-form-urlencoded")
            }))
            .with_post(Arc::new(
                |_, _, _, credential, headers, _, options, cancel| {
                    let Some((name, value)) = credential else {
                        panic!("the runtime credential was not attached")
                    };
                    assert_eq!(name, "Authorization");
                    assert!(
                        value.starts_with("Bearer ") && value.len() > "Bearer ".len(),
                        "the runtime did not apply bearer framing"
                    );
                    assert!(headers.is_empty(), "runtime controls leaked upstream");
                    assert!(options.report_rate_limit);
                    assert!(options.wait_until_cancelled);
                    assert!(!cancel.load(Ordering::SeqCst));
                    Ok(Vec::new())
                },
            ));
        runner
            .submit(
                TaskId(1),
                Task::Post {
                    url: "https://example.invalid/seek".into(),
                    body: "fixed=1".into(),
                    content_type: "application/x-www-form-urlencoded".into(),
                    credential: Some(Credential::bearer("openai")),
                    headers: vec![
                        Header::new("X-Cobalt-Wait-Until-Cancelled", "1"),
                        Header::new("X-Cobalt-Rate-Limit", "1"),
                    ],
                    max_bytes: 1024,
                },
            )
            .expect("submitted");
        assert_eq!(
            collect(&mut runner, 1)[0].outcome,
            TaskOutcome::Completed(Vec::new())
        );
    }

    #[test]
    fn line_stream_controls_round_trip_as_an_unchanged_protocol_11_fetch() {
        use kobo_protocol::{read_from, write_to, Frame, Message, LEGACY_VERSION};
        use std::io::Cursor;

        assert_eq!(LEGACY_VERSION, 11);
        let frame = Frame {
            version: LEGACY_VERSION,
            request_id: 9,
            message: Message::Spawn {
                task: TaskId(4),
                work: Task::Fetch {
                    url: "https://lichess.org/api/stream/event".into(),
                    offset: 0,
                    max_bytes: 4096,
                    credential: Some(Credential::bearer("lichess")),
                    headers: vec![
                        Header::new("Accept", "application/x-ndjson"),
                        Header::new("X-Cobalt-Line-Stream", "open"),
                    ],
                },
            },
        };
        let mut encoded = Vec::new();
        write_to(&mut encoded, &frame).expect("encode protocol 11");
        let decoded = read_from(&mut Cursor::new(encoded)).expect("decode protocol 11");
        assert_eq!(decoded, frame);
    }

    #[test]
    fn cancelling_open_or_next_closes_a_stream_but_cancelling_close_does_not() {
        let work = |action| Task::Fetch {
            url: "https://lichess.org/api/stream/event".into(),
            offset: 0,
            max_bytes: 4096,
            credential: Some(Credential::bearer("lichess")),
            headers: vec![
                Header::new("Accept", "application/x-ndjson"),
                Header::new("X-Cobalt-Line-Stream", action),
            ],
        };
        assert!(line_stream_url(&work("open")).is_some());
        assert!(line_stream_url(&work("next")).is_some());
        assert!(line_stream_url(&work("close")).is_none());
    }

    #[test]
    fn unknown_duplicate_or_misplaced_runtime_controls_fail_closed() {
        for headers in [
            vec![Header::new("X-Cobalt-Unknown", "1")],
            vec![
                Header::new("X-Cobalt-Rate-Limit", "1"),
                Header::new("x-cobalt-rate-limit", "1"),
            ],
            vec![Header::new("X-Cobalt-Wait-Until-Cancelled", "1")],
            vec![
                Header::new("Accept", "application/x-ndjson"),
                Header::new("X-Cobalt-Line-Stream", "open"),
            ],
        ] {
            let mut runner = TaskRunner::simulated(temp_root("bad-controls"))
                .with_capabilities([Capability::Network])
                .with_fetch(Arc::new(|_, _, _, _, _, _, _| {
                    panic!("a rejected control must not reach the network")
                }));
            runner
                .submit(
                    TaskId(1),
                    Task::Fetch {
                        url: "https://example.invalid/".into(),
                        offset: 0,
                        max_bytes: 128,
                        credential: None,
                        headers,
                    },
                )
                .expect("submitted");
            assert_eq!(
                collect(&mut runner, 1)[0].outcome,
                TaskOutcome::Failed(TaskError::Denied)
            );
        }
    }

    #[test]
    fn an_application_supplied_range_header_cannot_displace_the_byte_offset_the_runtime_sets() {
        // `offset` is the only thing allowed to become `Range`: it is turned
        // into the header the runtime sends, not merged with one the
        // application names, so a fetch naming its own `Range` is refused
        // outright rather than sent with two.
        let mut runner = TaskRunner::simulated(temp_root("fetch-range"))
            .with_capabilities([Capability::Network])
            .with_fetch(Arc::new(|_, _, _, _, _, _, _| {
                Ok(b"should not run".to_vec())
            }));
        runner
            .submit(
                TaskId(1),
                Task::Fetch {
                    url: "https://example.test/book.txt".into(),
                    offset: 100,
                    max_bytes: 128,
                    credential: None,
                    headers: vec![Header::new("Range", "bytes=0-9999")],
                },
            )
            .expect("submitted");
        let finished = collect(&mut runner, 1);
        assert_eq!(finished[0].outcome, TaskOutcome::Failed(TaskError::Denied));
    }

    fn secret_dir(name: &str) -> PathBuf {
        let path = temp_root(&format!("secrets-{name}"));
        let _ = std::fs::write(path.join("openai"), "not-a-real-key\n");
        path
    }

    #[test]
    fn a_secret_name_cannot_walk_out_of_the_secret_directory() {
        // The application chooses this name and also chooses the server the
        // value is sent to, so a name that escaped the directory would be a
        // way to post any readable file on the device to anywhere.
        let directory = secret_dir("escape");
        for name in [
            "../../etc/passwd",
            "..",
            ".",
            "sub/openai",
            "openai\0",
            "",
            "open ai",
        ] {
            assert_eq!(
                secret(Some(&directory), name),
                None,
                "{name} was allowed out of the directory"
            );
        }
        assert_eq!(
            secret(Some(&directory), "openai").as_deref(),
            Some("not-a-real-key")
        );
    }

    #[test]
    fn a_runner_with_no_secret_directory_holds_no_secrets() {
        // The simulator runs on a development machine and must never be able
        // to reach a real credential, however an application asks.
        assert_eq!(secret(None, "openai"), None);
    }

    #[test]
    fn app_scoped_credentials_precede_global_owner_credentials() {
        let root = temp_root("app-secret-precedence");
        std::fs::write(root.join("openai"), "owner-global").expect("global secret");
        crate::credentials::install_app_secret(&root, "chat", "openai", "chat-secret")
            .expect("chat secret");
        crate::credentials::install_app_secret(&root, "audiobook", "openai", "audiobook-secret")
            .expect("audiobook secret");
        let chat = SecretStore {
            root: root.clone(),
            app: Some("chat".to_owned()),
        };
        let audiobook = SecretStore {
            root: root.clone(),
            app: Some("audiobook".to_owned()),
        };
        let other = SecretStore {
            root,
            app: Some("zotero-reader".to_owned()),
        };
        assert_eq!(
            scoped_secret(Some(&chat), "openai").as_deref(),
            Some("chat-secret")
        );
        assert_eq!(
            scoped_secret(Some(&audiobook), "openai").as_deref(),
            Some("audiobook-secret")
        );
        assert_eq!(
            scoped_secret(Some(&other), "openai").as_deref(),
            Some("owner-global"),
            "legacy and CLI-installed globals remain the deliberate fallback"
        );

        let legacy = temp_root("legacy-global-secret");
        std::fs::write(legacy.join("openai"), "legacy-global").expect("legacy secret");
        for app in ["chat", "audiobook"] {
            let store = SecretStore {
                root: legacy.clone(),
                app: Some(app.to_owned()),
            };
            assert_eq!(
                scoped_secret(Some(&store), "openai").as_deref(),
                Some("legacy-global"),
                "{app} lost the pre-namespace credential"
            );
        }
    }

    #[test]
    fn app_secret_lookup_refuses_symlinked_namespaces_and_values() {
        use std::os::unix::fs::symlink;

        let root = temp_root("app-secret-symlink");
        std::fs::create_dir_all(root.join("apps/chat")).expect("chat namespace");
        std::fs::write(root.join("outside"), "must-not-be-read").expect("outside");
        symlink(root.join("outside"), root.join("apps/chat/openai")).expect("secret link");
        let store = SecretStore {
            root,
            app: Some("chat".to_owned()),
        };
        assert_eq!(scoped_secret(Some(&store), "openai"), None);
    }

    #[test]
    fn an_oversized_file_is_not_treated_as_a_secret() {
        // A log or a truncated download dropped in the directory is not a
        // credential, and sending one to a server would leak whatever it held.
        let directory = temp_root("oversized");
        let _ = std::fs::write(directory.join("big"), "x".repeat(MAX_SECRET_BYTES + 1));
        assert_eq!(secret(Some(&directory), "big"), None);
    }

    #[test]
    fn a_post_naming_a_secret_the_runtime_does_not_hold_is_refused_rather_than_sent() {
        // Sending it anyway would reach the server unauthenticated, and the
        // application would then report the server's complaint about that
        // instead of the real problem, which is a missing credential.
        let sent = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&sent);
        let mut runner = TaskRunner::simulated(temp_root("nosecret"))
            .with_capabilities([Capability::Network])
            .with_secrets(temp_root("nosecret-empty"))
            .with_credential_policy(Arc::new(|_, _, _, _, _| true))
            .with_post(Arc::new(move |_, _, _, _, _, _, _, _| {
                observed.store(true, Ordering::SeqCst);
                Ok(Vec::new())
            }));
        runner
            .submit(
                TaskId(1),
                Task::Post {
                    url: String::from("https://example.invalid/"),
                    body: String::from("{}"),
                    content_type: String::from("application/json"),
                    credential: Some(Credential::bearer("absent")),
                    headers: Vec::new(),
                    max_bytes: 1024,
                },
            )
            .expect("submitted");
        let finished = collect(&mut runner, 1);
        // `NoCredential` rather than `Denied`: the application was allowed to
        // ask, so the refusal has to say the key is missing rather than accuse
        // the application of lacking a permission it actually holds.
        assert_eq!(
            finished[0].outcome,
            TaskOutcome::Failed(TaskError::NoCredential)
        );
        assert!(!sent.load(Ordering::SeqCst), "the request was sent anyway");
    }

    #[test]
    fn the_application_never_sees_the_value_of_the_secret_it_named() {
        // The whole point of naming a secret rather than carrying one. What
        // comes back is the server's answer and nothing else.
        let directory = secret_dir("resolved");
        let mut runner = TaskRunner::simulated(temp_root("resolved-root"))
            .with_capabilities([Capability::Network])
            .with_secrets(directory)
            .with_credential_policy(Arc::new(|credential, url, _, _, _| {
                credential.secret == "openai" && url == "https://example.invalid/"
            }))
            .with_post(Arc::new(|_, _, _, credential, _, _, _, _| {
                assert_eq!(credential, Some(("Authorization", "Bearer not-a-real-key")));
                Ok(b"{\"ok\":true}".to_vec())
            }));
        runner
            .submit(
                TaskId(1),
                Task::Post {
                    url: String::from("https://example.invalid/"),
                    body: String::from("{}"),
                    content_type: String::from("application/json"),
                    credential: Some(Credential::bearer("openai")),
                    headers: Vec::new(),
                    max_bytes: 1024,
                },
            )
            .expect("submitted");
        let finished = collect(&mut runner, 1);
        let TaskOutcome::Completed(bytes) = &finished[0].outcome else {
            panic!("expected a completed task, got {:?}", finished[0].outcome);
        };
        let answer = String::from_utf8_lossy(bytes);
        assert_eq!(answer, "{\"ok\":true}");
        assert!(!answer.contains("not-a-real-key"));
    }

    #[test]
    fn removing_a_secret_denies_the_next_request_without_calling_the_backend() {
        let directory = secret_dir("removed");
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let mut runner = TaskRunner::simulated(temp_root("removed-root"))
            .with_capabilities([Capability::Network])
            .with_secrets(&directory)
            .with_credential_policy(Arc::new(|_, _, _, _, _| true))
            .with_fetch(Arc::new(move |_, _, _, _, _, _, _| {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(Vec::new())
            }));
        let work = || Task::Fetch {
            url: "https://example.invalid/".into(),
            offset: 0,
            max_bytes: 128,
            credential: Some(Credential::bearer("openai")),
            headers: Vec::new(),
        };
        runner.submit(TaskId(1), work()).expect("first");
        assert!(matches!(
            collect(&mut runner, 1)[0].outcome,
            TaskOutcome::Completed(_)
        ));
        std::fs::remove_file(directory.join("openai")).expect("remove secret");
        runner.submit(TaskId(2), work()).expect("second");
        assert_eq!(
            collect(&mut runner, 1)[0].outcome,
            TaskOutcome::Failed(TaskError::NoCredential)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_secret_cannot_be_sent_to_a_destination_its_policy_did_not_allow() {
        let directory = secret_dir("wrong-destination");
        let sent = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&sent);
        let mut runner = TaskRunner::simulated(temp_root("wrong-destination-root"))
            .with_capabilities([Capability::Network])
            .with_secrets(directory)
            .with_credential_policy(Arc::new(|credential, url, _, _, _| {
                credential.secret == "openai" && url == "https://api.openai.com/v1/chat/completions"
            }))
            .with_post(Arc::new(move |_, _, _, _, _, _, _, _| {
                observed.store(true, Ordering::SeqCst);
                Ok(Vec::new())
            }));
        runner
            .submit(
                TaskId(1),
                Task::Post {
                    url: "https://attacker.invalid/collect".into(),
                    body: "{}".into(),
                    content_type: "application/json".into(),
                    credential: Some(Credential::bearer("openai")),
                    headers: Vec::new(),
                    max_bytes: 1024,
                },
            )
            .expect("submitted");
        assert_eq!(
            collect(&mut runner, 1)[0].outcome,
            TaskOutcome::Failed(TaskError::Denied)
        );
        assert!(!sent.load(Ordering::SeqCst));
    }

    #[test]
    fn credential_policy_distinguishes_fetch_from_post_to_the_same_url() {
        let directory = secret_dir("read-only-method");
        let mut runner = TaskRunner::simulated(temp_root("read-only-method-root"))
            .with_capabilities([Capability::Network])
            .with_secrets(directory)
            .with_credential_policy(Arc::new(|credential, url, usage, _, _| {
                credential.secret == "openai"
                    && url == "https://example.invalid/items"
                    && usage == CredentialUse::Fetch
            }))
            .with_fetch(Arc::new(|_, _, _, credential, headers, _, _| {
                assert_eq!(credential, Some(("x-api-key", "not-a-real-key")));
                assert!(headers.is_empty());
                Ok(b"[]".to_vec())
            }))
            .with_post(Arc::new(|_, _, _, _, _, _, _, _| Ok(b"{}".to_vec())));
        runner
            .submit(
                TaskId(1),
                Task::Fetch {
                    url: "https://example.invalid/items".into(),
                    offset: 0,
                    max_bytes: 1024,
                    credential: Some(Credential::in_header("openai", "x-api-key")),
                    headers: Vec::new(),
                },
            )
            .expect("fetch submitted");
        assert!(matches!(
            collect(&mut runner, 1)[0].outcome,
            TaskOutcome::Completed(_)
        ));

        runner
            .submit(
                TaskId(2),
                Task::Post {
                    url: "https://example.invalid/items".into(),
                    body: "[]".into(),
                    content_type: "application/json".into(),
                    credential: Some(Credential::bearer("openai")),
                    headers: Vec::new(),
                    max_bytes: 1024,
                },
            )
            .expect("post submitted");
        assert_eq!(
            collect(&mut runner, 1)[0].outcome,
            TaskOutcome::Failed(TaskError::Denied)
        );
    }

    #[test]
    fn posting_without_the_network_capability_is_refused() {
        let mut runner = TaskRunner::simulated(temp_root("ungranted")).with_post(Arc::new(
            |_, _, _, _, _, _, _, _| panic!("an ungranted post must never reach the network"),
        ));
        runner
            .submit(
                TaskId(1),
                Task::Post {
                    url: String::from("https://example.invalid/"),
                    body: String::from("{}"),
                    content_type: String::from("application/json"),
                    credential: None,
                    headers: Vec::new(),
                    max_bytes: 1024,
                },
            )
            .expect("submitted");
        assert_eq!(
            collect(&mut runner, 1)[0].outcome,
            TaskOutcome::Failed(TaskError::Denied)
        );
    }
    #[test]
    fn a_basic_credential_is_encoded_by_the_runtime_rather_than_the_application() {
        // The pairs from RFC 4648, plus the shape Standard Ebooks asks for:
        // an address as the user and nothing at all as the password.
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(
            base64(b"reader@example.com:"),
            "cmVhZGVyQGV4YW1wbGUuY29tOg=="
        );
    }
}
