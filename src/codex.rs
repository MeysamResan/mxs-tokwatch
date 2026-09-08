//! Read-only account access through the official Codex app-server protocol.
//!
//! Authentication belongs to Codex. This module never reads auth files, sends
//! prompts, starts threads, redeems resets, or logs protocol payloads.

use crate::model::UsageSnapshot;
use serde_json::{Value, json};
use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::{io::AsRawHandle, process::CommandExt};
#[cfg(windows)]
use windows::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    },
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
const REPLY_QUEUE_CAPACITY: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountStatus {
    SignedIn { label: String },
    SignedOut,
    ApiKey,
}

/// Finds a native executable without running a shell or an npm/PowerShell shim.
pub fn discover_codex() -> Result<PathBuf, String> {
    let path_dirs: Vec<PathBuf> = env::var_os("PATH")
        .map(|path| {
            env::split_paths(&path)
                .filter(|p| p.is_absolute())
                .collect()
        })
        .unwrap_or_default();

    // Native PATH installations take precedence over npm wrapper discovery.
    for directory in &path_dirs {
        if let Some(path) = executable_in(directory) {
            return Ok(path);
        }
    }

    let mut npm_dirs = path_dirs;
    if let Some(roaming) = env::var_os("APPDATA") {
        npm_dirs.push(PathBuf::from(roaming).join("npm"));
    }
    for directory in npm_dirs {
        if let Some(path) = find_npm_binary(&directory) {
            return Ok(path);
        }
    }

    // The desktop app installs versioned native helpers in this directory.
    // Choose the most recently written executable, never the hexadecimal name.
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        let bin = PathBuf::from(local).join("OpenAI/Codex/bin");
        if let Ok(entries) = bin.read_dir() {
            let mut candidates: Vec<_> = entries
                .filter_map(Result::ok)
                .filter_map(|entry| executable_in(&entry.path()))
                .collect();
            candidates.sort_by_key(|path| {
                std::cmp::Reverse(path.metadata().and_then(|m| m.modified()).ok())
            });
            if let Some(path) = candidates.into_iter().next() {
                return Ok(path);
            }
        }
    }

    Err("Codex was not found. Install the Windows Codex CLI or select its native codex.exe in settings.".into())
}

fn executable_in(directory: &Path) -> Option<PathBuf> {
    let path = directory.join(if cfg!(windows) { "codex.exe" } else { "codex" });
    path.is_file().then_some(path)
}

fn find_npm_binary(prefix: &Path) -> Option<PathBuf> {
    let targets = if cfg!(target_arch = "aarch64") {
        [("arm64", "aarch64"), ("x64", "x86_64")]
    } else {
        [("x64", "x86_64"), ("arm64", "aarch64")]
    };
    let root = prefix.join("node_modules/@openai/codex");
    for (package_arch, triple_arch) in targets {
        let platform_package = format!("@openai/codex-win32-{package_arch}");
        let native_path = format!("vendor/{triple_arch}-pc-windows-msvc/bin/codex.exe");
        // npm can install optional platform dependencies nested or hoisted.
        // Older Codex releases used a `codex` subdirectory instead of `bin`.
        for package in [
            root.join("node_modules").join(&platform_package),
            prefix.join("node_modules").join(&platform_package),
            root.clone(),
        ] {
            for relative in [native_path.clone(), native_path.replace("/bin/", "/codex/")] {
                let path = package.join(relative);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// Keep one account helper between refreshes instead of repeatedly interrupting
/// its startup and Windows authentication/IPC cleanup.
#[derive(Default)]
pub struct Session {
    active: Option<(PathBuf, Client)>,
    stopping: Arc<AtomicBool>,
}

impl Session {
    pub fn new(stopping: Arc<AtomicBool>) -> Self {
        Self {
            active: None,
            stopping,
        }
    }

    pub fn connect(&mut self, path: Option<PathBuf>) -> Result<&mut Client, String> {
        let path = match path {
            Some(path) => path,
            None => discover_codex()?,
        };
        let stopping = Arc::clone(&self.stopping);
        self.get_or_spawn(&path, |path| Client::spawn(path, stopping))
    }

    fn get_or_spawn(
        &mut self,
        path: &Path,
        spawn: impl FnOnce(&Path) -> Result<Client, String>,
    ) -> Result<&mut Client, String> {
        let reusable = self.active.as_mut().is_some_and(|(active_path, client)| {
            active_path == path && !client.stopped && matches!(client.child.try_wait(), Ok(None))
        });
        if !reusable {
            self.clear();
            self.active = Some((path.to_owned(), spawn(path)?));
        }
        Ok(&mut self.active.as_mut().expect("connected helper").1)
    }

    pub fn client(&mut self) -> Option<&mut Client> {
        self.active.as_mut().map(|(_, client)| client)
    }

    pub fn clear(&mut self) {
        self.active = None;
    }
}

pub struct Client {
    child: Child,
    input: Option<ChildStdin>,
    replies: Receiver<Result<Value, String>>,
    reader: Option<JoinHandle<()>>,
    next_id: u64,
    stopped: bool,
    stopping: Arc<AtomicBool>,
    login_id: Option<String>,
    login_completion: Option<(String, bool)>,
    #[cfg(windows)]
    job: Option<ChildJob>,
}

impl Client {
    pub fn spawn(path: &Path, stopping: Arc<AtomicBool>) -> Result<Self, String> {
        if !path.is_absolute() || !path.is_file() {
            return Err("Select an existing, absolute path to the native Codex executable.".into());
        }
        #[cfg(windows)]
        if !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
        {
            return Err("Select the native codex.exe, rather than a .cmd or .ps1 launcher.".into());
        }

        let mut command = Command::new(path);
        command.args(["app-server", "--listen", "stdio://"]);

        // Avoid picking up arbitrary project configuration from the app's launch
        // directory. Authentication still uses Codex's own configured home.
        if let Some(home) = env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
        {
            command.current_dir(home);
        }

        Self::spawn_command(command, stopping)
    }

    fn spawn_command(mut command: Command, stopping: Arc<AtomicBool>) -> Result<Self, String> {
        if stopping.load(Ordering::Acquire) {
            return Err("The Codex connection is closing.".into());
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // App-server diagnostics may contain account or authentication data.
            // Discard them and report only locally generated errors below.
            .stderr(Stdio::null());
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

        let mut child = command.spawn().map_err(|error| {
            format!(
                "Could not start Codex ({}). Check the executable in settings.",
                error.kind()
            )
        })?;

        #[cfg(windows)]
        let job = match ChildJob::attach(&child) {
            Ok(job) => Some(job),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };

        let Some(input) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Could not open the Codex input connection.".into());
        };
        let Some(output) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Could not open the Codex output connection.".into());
        };
        let (sender, replies) = mpsc::sync_channel(REPLY_QUEUE_CAPACITY);
        let reader = match thread::Builder::new()
            .name("codex-reader".into())
            .spawn(move || read_messages(BufReader::new(output), sender))
        {
            Ok(reader) => reader,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Could not start the Codex response reader.".into());
            }
        };
        let mut client = Self {
            child,
            input: Some(input),
            replies,
            reader: Some(reader),
            next_id: 1,
            stopped: false,
            stopping,
            login_id: None,
            login_completion: None,
            #[cfg(windows)]
            job,
        };
        client.request(
            "initialize",
            Some(json!({
                "clientInfo": {
                    "name": "mxs_tokwatch",
                    "title": "TokWatch",
                    "version": env!("CARGO_PKG_VERSION")
                }
            })),
        )?;
        client.send(&json!({ "method": "initialized" }))?;
        Ok(client)
    }

    pub fn read_account(&mut self) -> Result<AccountStatus, String> {
        let result = self.request("account/read", Some(json!({ "refreshToken": false })))?;
        parse_account(&result)
    }

    pub fn read_limits(&mut self) -> Result<UsageSnapshot, String> {
        let result = self.request("account/rateLimits/read", None)?;
        UsageSnapshot::from_response(result)
    }

    /// Starts the Codex-managed browser flow. Only call after the user clicks
    /// Connect; keep this Client alive until sign-in completes or is cancelled.
    pub fn start_login(&mut self) -> Result<String, String> {
        self.login_id = None;
        self.login_completion = None;
        let result = self.request("account/login/start", Some(json!({ "type": "chatgpt" })))?;
        self.login_id = Some(
            result
                .get("loginId")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    "Codex did not return a browser sign-in session. Update Codex and try again."
                        .to_owned()
                })?
                .to_owned(),
        );
        let url = result
            .get("authUrl")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                "Codex did not return a browser sign-in link. Update Codex and try again."
                    .to_owned()
            })?;
        validate_login_url(url)?;
        Ok(url.to_owned())
    }

    /// Checks the explicit login-completed event without issuing a request.
    /// Cached account identity can still represent an older login, so it must
    /// never be treated as proof that a pending browser flow has completed.
    pub fn poll_login(&mut self) -> Result<bool, String> {
        if self.login_id.is_none() {
            return Err("No Codex browser sign-in is pending. Select Connect to try again.".into());
        }
        for _ in 0..REPLY_QUEUE_CAPACITY {
            match self.replies.try_recv() {
                Ok(Ok(message)) => {
                    self.handle_server_message(&message)?;
                }
                Ok(Err(error)) => {
                    self.stop();
                    return Err(error);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.stop();
                    return Err(
                        "Codex closed the sign-in connection. Select Connect to try again.".into(),
                    );
                }
            }
        }
        match self.login_completion.as_ref() {
            Some((id, success)) if Some(id) == self.login_id.as_ref() => {
                if *success {
                    Ok(true)
                } else {
                    Err(
                        "Codex browser sign-in did not complete. Select Connect to try again."
                            .into(),
                    )
                }
            }
            _ => Ok(false),
        }
    }

    fn handle_server_message(&mut self, message: &Value) -> Result<bool, String> {
        if message.get("method").and_then(Value::as_str) == Some("account/login/completed") {
            if let Some(completion) = parse_login_completion(message, self.login_id.as_deref()) {
                self.login_completion = Some(completion);
            }
            return Ok(true);
        }
        if message.get("method").is_some() {
            if let Some(id) = message.get("id") {
                // Never approve unexpected requests or supply credentials.
                self.send(&json!({
                    "id": id,
                    "error": { "code": -32601, "message": "TokWatch does not support server requests" }
                }))?;
            }
            return Ok(true);
        }
        Ok(false)
    }

    fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value, String> {
        if self.stopping.load(Ordering::Acquire) {
            self.stop();
            return Err("The Codex connection is closing.".into());
        }
        if self.stopped {
            return Err("The Codex connection closed. Refresh to reconnect.".into());
        }
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({ "id": id, "method": method });
        if let Some(params) = params {
            message["params"] = params;
        }
        self.send(&message)?;
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        loop {
            if self.stopping.load(Ordering::Acquire) {
                self.stop();
                return Err("The Codex connection is closing.".into());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let reply = match self
                .replies
                .recv_timeout(remaining.min(Duration::from_millis(100)))
            {
                Ok(Ok(reply)) => reply,
                Ok(Err(error)) => {
                    self.stop();
                    return Err(error);
                }
                Err(RecvTimeoutError::Timeout) if Instant::now() < deadline => continue,
                Err(RecvTimeoutError::Timeout) => {
                    self.stop();
                    return Err("Codex did not respond within 30 seconds. Check your connection and refresh.".into());
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.stop();
                    return Err("Codex closed the connection. Update Codex or check its configuration, then refresh.".into());
                }
            };
            if self.handle_server_message(&reply)? {
                continue;
            }
            if reply.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = reply.get("error") {
                return Err(rpc_error(method, error));
            }
            return reply.get("result").cloned().ok_or_else(|| {
                "Codex returned an incomplete response. Update Codex and refresh.".into()
            });
        }
    }

    fn send(&mut self, value: &Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(value)
            .map_err(|_| "Could not prepare the Codex request.".to_owned())?;
        bytes.push(b'\n');
        let Some(input) = self.input.as_mut() else {
            return Err("The Codex connection is closed.".into());
        };
        if input.write_all(&bytes).and_then(|_| input.flush()).is_err() {
            self.stop();
            return Err("Could not communicate with Codex. Refresh to reconnect.".into());
        }
        Ok(())
    }

    fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        // EOF lets a cooperative stdio helper finish its own cleanup. Keep the
        // job alive during this grace period; closing it first kills the helper.
        self.input.take();
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        let exited = loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break true,
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(20));
                }
                _ => break false,
            }
        };
        // Only an unresponsive helper needs forced termination. The job also
        // closes stdout inherited by descendants before the reader is joined.
        #[cfg(windows)]
        self.job.take();
        if !exited {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stop();
    }
}

fn parse_login_completion(message: &Value, expected_id: Option<&str>) -> Option<(String, bool)> {
    let id = message.pointer("/params/loginId")?.as_str()?;
    let success = message.pointer("/params/success")?.as_bool()?;
    expected_id
        .is_none_or(|expected| expected == id)
        .then(|| (id.to_owned(), success))
}

fn parse_account(result: &Value) -> Result<AccountStatus, String> {
    let account = result.get("account").ok_or_else(|| {
        "Codex returned an incomplete account response. Update Codex and refresh.".to_owned()
    })?;
    if account.is_null() {
        return Ok(AccountStatus::SignedOut);
    }
    match account.get("type").and_then(Value::as_str) {
        Some("chatgpt" | "chatgptAuthTokens") => {
            // The tray only needs the plan, so do not retain the email address.
            let plan = account.get("planType").and_then(Value::as_str)
                .unwrap_or("").chars().filter(|c| c.is_alphanumeric() || *c == ' ')
                .take(40).collect::<String>();
            let label = if plan.is_empty() {
                "ChatGPT account".into()
            } else {
                let mut chars = plan.chars();
                let title = chars.next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                format!("ChatGPT {title}{}", chars.as_str())
            };
            Ok(AccountStatus::SignedIn { label })
        }
        Some("apiKey" | "apikey") => Ok(AccountStatus::ApiKey),
        _ => Err("This Codex account type does not provide a ChatGPT subscription allowance. Connect with ChatGPT.".into()),
    }
}

fn validate_login_url(url: &str) -> Result<(), String> {
    // Only open official HTTPS sign-in origins; reject credentials, ports, URL
    // control characters, or a scheme that could execute a local application.
    let authority = url
        .strip_prefix("https://")
        .and_then(|rest| rest.split(['/', '?', '#']).next());
    if url.len() <= 16_384
        && !url.chars().any(|c| c.is_control() || c == '\\')
        && matches!(
            authority,
            Some("auth.openai.com" | "auth0.openai.com" | "chatgpt.com")
        )
    {
        Ok(())
    } else {
        Err("Codex returned an unsupported sign-in link. Update Codex and try again.".into())
    }
}

fn rpc_error(method: &str, error: &Value) -> String {
    let code = error.get("code").and_then(Value::as_i64);
    // Classify messages in memory, but never expose arbitrary backend text: it
    // can include identifiers, request headers, or authentication material.
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    if message.contains("unauthoriz")
        || message.contains("401")
        || message.contains("not logged")
        || message.contains("not authenticated")
    {
        "Codex needs a ChatGPT sign-in. Select Connect to Codex.".into()
    } else if code == Some(-32601) || code == Some(-32602) {
        "This Codex version does not support the required account interface. Update Codex and refresh.".into()
    } else if message.contains("429") || message.contains("too many requests") {
        "The usage service is busy. TokWatch will retry after a pause.".into()
    } else if message.contains("api key") || message.contains("apikey") {
        "An API key does not provide subscription usage. Connect with your ChatGPT account.".into()
    } else if method == "account/login/start" {
        "Codex could not start browser sign-in. Close any other pending Codex sign-in and try again.".into()
    } else {
        "Codex could not retrieve account information. Check your connection, then refresh.".into()
    }
}

fn read_messages<R: BufRead>(mut reader: R, sender: SyncSender<Result<Value, String>>) {
    loop {
        let mut bytes = Vec::new();
        let read = reader
            .by_ref()
            .take(MAX_MESSAGE_BYTES as u64 + 1)
            .read_until(b'\n', &mut bytes);
        match read {
            Ok(0) => break,
            Ok(_) if bytes.len() > MAX_MESSAGE_BYTES => {
                let _ = sender.try_send(Err(
                    "Codex returned an oversized response. Update Codex and refresh.".into(),
                ));
                break;
            }
            Err(_) => {
                let _ = sender.try_send(Err(
                    "Could not read the Codex response. Refresh to reconnect.".into(),
                ));
                break;
            }
            Ok(_) => {}
        }
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(message)
                if message.get("id").is_some()
                    || message.get("method").and_then(Value::as_str)
                        == Some("account/login/completed") =>
            {
                // A single request is outstanding. Never let a broken child
                // flood memory or block the reader on an unconsumed queue.
                if sender.try_send(Ok(message)).is_err() {
                    break;
                }
            }
            Ok(_) => {} // Only browser login completion notifications are retained.
            Err(_) => {
                let _ = sender.try_send(Err(
                    "Codex returned an invalid response. Update Codex and refresh.".into(),
                ));
                break;
            }
        }
    }
}

#[cfg(windows)]
struct ChildJob(HANDLE);

#[cfg(windows)]
impl ChildJob {
    fn attach(child: &Child) -> Result<Self, String> {
        unsafe {
            let handle = CreateJobObjectW(None, None)
                .map_err(|_| "Could not create the Codex helper lifetime guard.".to_owned())?;
            let job = Self(handle);
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            )
            .map_err(|_| "Could not configure the Codex helper lifetime guard.".to_owned())?;
            AssignProcessToJobObject(handle, HANDLE(child.as_raw_handle()))
                .map_err(|_| "Could not attach the Codex helper lifetime guard.".to_owned())?;
            Ok(job)
        }
    }
}

#[cfg(windows)]
impl Drop for ChildJob {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn distinguishes_signed_out_api_key_and_chatgpt() {
        assert_eq!(
            parse_account(&json!({"account": null})).unwrap(),
            AccountStatus::SignedOut
        );
        assert_eq!(
            parse_account(&json!({"account": {"type": "apiKey"}})).unwrap(),
            AccountStatus::ApiKey
        );
        assert_eq!(
            parse_account(&json!({"account": {"type": "chatgpt", "email": null}})).unwrap(),
            AccountStatus::SignedIn {
                label: "ChatGPT account".into()
            }
        );
        assert!(parse_account(&json!({})).is_err());
        assert!(parse_account(&json!({"account": {"type": "amazonBedrock"}})).is_err());
    }

    #[test]
    fn only_opens_official_https_sign_in_links() {
        for good in [
            "https://auth.openai.com/authorize?state=example",
            "https://chatgpt.com/auth/login",
            "https://auth0.openai.com/authorize",
        ] {
            assert!(validate_login_url(good).is_ok());
        }
        for bad in [
            "file:///C:/app.exe",
            "https://chatgpt.com.evil.test/",
            "https://chatgpt.com@evil.test/",
            "https://chatgpt.com\\@evil.test",
            "https://chatgpt.com\r\n",
            "http://auth.openai.com/",
        ] {
            assert!(validate_login_url(bad).is_err());
        }
    }

    #[test]
    fn drops_notifications_and_keeps_rpc_responses() {
        let (sender, receiver) = mpsc::sync_channel(REPLY_QUEUE_CAPACITY);
        read_messages(
            Cursor::new(
                b"{\"method\":\"account/updated\",\"params\":{}}\n{\"id\":7,\"result\":{}}\n",
            ),
            sender,
        );
        assert_eq!(receiver.recv().unwrap().unwrap()["id"], 7);
        assert!(receiver.recv().is_err());
    }

    #[test]
    fn bounds_protocol_frames_and_rejects_invalid_json() {
        let (sender, receiver) = mpsc::sync_channel(REPLY_QUEUE_CAPACITY);
        read_messages(Cursor::new(vec![b'x'; MAX_MESSAGE_BYTES + 1]), sender);
        assert!(receiver.recv().unwrap().unwrap_err().contains("oversized"));
        let (sender, receiver) = mpsc::sync_channel(REPLY_QUEUE_CAPACITY);
        read_messages(Cursor::new(b"private diagnostic text\n"), sender);
        let error = receiver.recv().unwrap().unwrap_err();
        assert!(error.contains("invalid"));
        assert!(!error.contains("private"));
    }

    #[test]
    fn sign_in_completion_must_match_the_current_browser_flow() {
        let old = json!({"params": {"loginId": "old", "success": true}});
        assert!(parse_login_completion(&old, Some("current")).is_none());
        let success = json!({"params": {"loginId": "current", "success": true}});
        assert_eq!(
            parse_login_completion(&success, Some("current")),
            Some(("current".into(), true))
        );
        // The server may send completion before the start response arrives.
        assert_eq!(
            parse_login_completion(&success, None),
            Some(("current".into(), true))
        );
        let failure =
            json!({"params": {"loginId": "current", "success": false, "error": "private details"}});
        assert_eq!(
            parse_login_completion(&failure, Some("current")),
            Some(("current".into(), false))
        );
        assert!(
            parse_login_completion(&json!({"account":{"type":"chatgpt"}}), Some("current"))
                .is_none()
        );
    }

    #[test]
    fn retains_only_browser_login_completion_notifications() {
        let frames = concat!(
            "{\"method\":\"account/updated\",\"params\":{\"authMode\":\"chatgpt\"}}\n",
            "{\"method\":\"account/login/completed\",\"params\":{\"loginId\":\"current\",\"success\":true}}\n"
        );
        let (sender, receiver) = mpsc::sync_channel(REPLY_QUEUE_CAPACITY);
        read_messages(Cursor::new(frames), sender);
        let event = receiver.recv().unwrap().unwrap();
        assert_eq!(event["method"], "account/login/completed");
        assert_eq!(
            parse_login_completion(&event, Some("current")),
            Some(("current".into(), true))
        );
        assert!(receiver.recv().is_err());
    }

    #[test]
    fn backend_diagnostics_are_not_exposed() {
        let error = rpc_error(
            "account/rateLimits/read",
            &json!({"code": -32000, "message": "401 Authorization Bearer SECRET"}),
        );
        assert!(error.contains("sign-in"));
        assert!(!error.contains("SECRET"));
        assert!(!error.contains("Bearer"));
    }

    /// Explicit opt-in only: contacts Codex's account service, sends no prompts,
    /// starts no sessions, and never prints account identifiers or credentials.
    #[test]
    #[ignore = "requires an installed Codex executable and existing ChatGPT sign-in"]
    fn live_read() {
        let path = discover_codex().expect("native Codex installation");
        let mut client = Client::spawn(&path, Arc::default()).expect("Codex app-server handshake");
        assert!(matches!(
            client.read_account().expect("account read"),
            AccountStatus::SignedIn { .. }
        ));
        let _snapshot = client.read_limits().expect("usage read");
        let child_id = client.child.id();
        client.stop();
        assert!(
            client
                .child
                .try_wait()
                .expect("child exit status")
                .is_some()
        );
        println!(
            "Read-only Codex handshake, account, and usage queries succeeded; helper {child_id} was reaped."
        );
    }
}

#[cfg(all(test, windows))]
#[path = "codex_lifecycle_tests.rs"]
mod lifecycle_tests;
