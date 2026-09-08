//! Offline child-process tests. PowerShell only acts as a JSONL fixture: no
//! Codex executable, account, credentials, external service, or machine changes.
use super::*;

const FIXTURE: &str = r#"
$ErrorActionPreference = 'Stop'
while ($null -ne ($line = [Console]::ReadLine())) {
    $request = ConvertFrom-Json -InputObject $line
    if ($null -eq $request.id) { continue }
    $result = switch ($request.method) {
        'initialize' { @{ userAgent = 'TokWatch offline test' } }
        'account/read' { @{ account = @{ type = 'chatgpt'; planType = 'pro' } } }
        'account/rateLimits/read' {
            @{ rateLimits = @{ primary = @{ usedPercent = 25; windowDurationMins = 300; resetsAt = 2000000000 } } }
        }
        'fixture/block' { [Threading.Thread]::Sleep(20000); @{} }
        default { @{} }
    }
    [Console]::WriteLine((ConvertTo-Json -Compress -Depth 8 -InputObject @{ id = $request.id; result = $result }))
    if ($request.method -eq 'fixture/exit') { exit 23 }
}
"#;

fn fixture(hang_on_eof: bool, stopping: Arc<AtomicBool>) -> Client {
    let powershell = PathBuf::from(env::var_os("SystemRoot").expect("Windows folder"))
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let tail = if hang_on_eof {
        "[Threading.Thread]::Sleep(20000); exit 23"
    } else {
        // A distinctive exit status proves EOF cleanup ran before job closure.
        "[Threading.Thread]::Sleep(80); exit 23"
    };
    let mut command = Command::new(powershell);
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-WindowStyle",
        "Hidden",
        "-Command",
        &format!("{FIXTURE}\n{tail}"),
    ]);
    Client::spawn_command(command, stopping).expect("offline helper handshake")
}

#[test]
fn refreshes_reuse_one_helper_and_reconnect_after_path_change() {
    let first = Path::new(r"C:\TokWatch-fixture\first.exe");
    let second = Path::new(r"C:\TokWatch-fixture\second.exe");
    let mut session = Session::default();
    let mut expected_pid = None;
    for _ in 0..5 {
        let client = session
            .get_or_spawn(first, |_| Ok(fixture(false, Arc::default())))
            .unwrap();
        let pid = client.child.id();
        assert_eq!(*expected_pid.get_or_insert(pid), pid);
        assert!(matches!(
            client.read_account().unwrap(),
            AccountStatus::SignedIn { .. }
        ));
        let snapshot = client.read_limits().unwrap();
        assert_eq!(snapshot.pools[0].windows[0].used_percent, Some(25.0));
    }
    let client = session
        .get_or_spawn(second, |_| Ok(fixture(false, Arc::default())))
        .unwrap();
    assert_ne!(Some(client.child.id()), expected_pid);
    assert!(client.read_limits().is_ok());
    session.clear();
    assert!(session.client().is_none());
}

#[test]
fn an_exited_or_stopped_helper_is_replaced_on_the_next_refresh() {
    let path = Path::new(r"C:\TokWatch-fixture\codex.exe");
    let mut session = Session::default();
    let client = session
        .get_or_spawn(path, |_| Ok(fixture(false, Arc::default())))
        .unwrap();
    client.request("fixture/exit", None).unwrap();
    assert_eq!(client.child.wait().unwrap().code(), Some(23));
    let client = session
        .get_or_spawn(path, |_| Ok(fixture(false, Arc::default())))
        .unwrap();
    assert!(client.read_limits().is_ok());
    client.stop();
    let client = session
        .get_or_spawn(path, |_| Ok(fixture(false, Arc::default())))
        .unwrap();
    assert!(client.read_limits().is_ok());
}

#[test]
fn shutdown_allows_eof_cleanup_and_reaps_the_reader() {
    let mut client = fixture(false, Arc::default());
    client.stop();
    assert_eq!(client.child.try_wait().unwrap().unwrap().code(), Some(23));
    assert!(client.reader.is_none());
    assert!(client.job.is_none());
    client.stop();
    assert_eq!(client.child.try_wait().unwrap().unwrap().code(), Some(23));
}

#[test]
fn an_unresponsive_helper_is_reaped_after_the_shutdown_deadline() {
    let mut client = fixture(true, Arc::default());
    let started = Instant::now();
    client.stop();
    assert!(started.elapsed() >= SHUTDOWN_TIMEOUT);
    assert!(started.elapsed() < SHUTDOWN_TIMEOUT + Duration::from_secs(5));
    assert_ne!(client.child.try_wait().unwrap().unwrap().code(), Some(23));
    assert!(client.reader.is_none());
    assert!(client.job.is_none());
}

#[test]
fn cancellation_interrupts_an_outstanding_request_and_closes_the_helper() {
    let stopping = Arc::new(AtomicBool::new(false));
    let mut client = fixture(false, Arc::clone(&stopping));
    let cancel = thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        stopping.store(true, Ordering::Release);
    });
    let started = Instant::now();
    assert!(
        client
            .request("fixture/block", None)
            .unwrap_err()
            .contains("closing")
    );
    assert!(started.elapsed() < Duration::from_secs(7));
    cancel.join().unwrap();
    assert!(client.child.try_wait().unwrap().is_some());
    assert!(client.reader.is_none());
}

#[test]
fn a_failed_reconnection_leaves_no_previous_helper() {
    let mut session = Session::default();
    session
        .get_or_spawn(Path::new("first"), |_| Ok(fixture(false, Arc::default())))
        .unwrap();
    assert!(
        session
            .get_or_spawn(Path::new("second"), |_| Err("fixture failure".into()))
            .is_err()
    );
    assert!(session.client().is_none());
}
