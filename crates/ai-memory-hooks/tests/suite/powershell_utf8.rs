//! Windows PowerShell compatibility-hook transport regression tests.

#![cfg(windows)]

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate should live under crates/ai-memory-hooks")
        .to_path_buf()
}

fn header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn receive_one_request(listener: TcpListener) -> (String, Vec<u8>) {
    listener.set_nonblocking(true).unwrap();
    // A cold PowerShell start on a loaded windows-latest runner (process spawn +
    // JIT + dot-sourcing the hook lib) can take well over ten seconds before the
    // hook opens its connection; the 10s deadline was marginal and flaked. This
    // window only bounds *failure detection* — a working hook connects in a few
    // seconds regardless, and a genuinely broken hook is caught earlier by the
    // `output.status.success()` assertion — so a generous deadline removes the
    // false negative without masking a real break.
    let deadline = Instant::now() + Duration::from_secs(60);
    let (mut stream, _) = loop {
        match listener.accept() {
            Ok(connection) => break connection,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "PowerShell hook never connected");
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("mock hook server accept failed: {error}"),
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();

    let mut request = Vec::new();
    let (body_start, body_len) = loop {
        let mut chunk = [0_u8; 4096];
        let read = stream.read(&mut chunk).expect("read hook request");
        assert!(read > 0, "hook request ended before its headers");
        request.extend_from_slice(&chunk[..read]);
        if let Some(end) = header_end(&request) {
            let headers = String::from_utf8_lossy(&request[..end]);
            let len = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .expect("PowerShell request should carry Content-Length");
            break (end + 4, len);
        }
    };
    while request.len() < body_start + body_len {
        let mut chunk = [0_u8; 4096];
        let read = stream.read(&mut chunk).expect("read hook request body");
        assert!(read > 0, "hook request body ended early");
        request.extend_from_slice(&chunk[..read]);
    }

    stream
        .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .unwrap();
    let headers = String::from_utf8(request[..body_start - 4].to_vec()).unwrap();
    let body = request[body_start..body_start + body_len].to_vec();
    (headers, body)
}

#[test]
fn powershell_hook_posts_json_as_utf8_bytes() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    let receiver = thread::spawn(move || receive_one_request(listener));

    let payload = serde_json::json!({
        "session_id": "utf8-regression",
        "prompt": "中文记忆测试：行动完成 — Validação: não, memória, correção, ação"
    })
    .to_string();
    let script = repo_root()
        .join("hooks")
        .join("lib")
        .join("ai-memory-hook.ps1");
    let script = script.to_string_lossy().replace('\'', "''");
    let program = format!(
        ". '{script}'; function Read-AiMemoryStdin {{ $env:AI_MEMORY_TEST_PAYLOAD }}; \
         Invoke-AiMemoryHook -Event 'user-prompt' -Agent 'codex'"
    );
    let output = Command::new(ai_memory_test_support::powershell_exe())
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &program,
        ])
        .env("AI_MEMORY_HOOK_URL", server)
        .env("AI_MEMORY_TEST_PAYLOAD", &payload)
        .output()
        .expect("run PowerShell hook");
    assert!(
        output.status.success(),
        "PowerShell hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let (headers, body) = receiver.join().unwrap();
    assert!(
        headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("Content-Type: application/json; charset=utf-8")),
        "request did not declare UTF-8 JSON: {headers}"
    );
    assert_eq!(body, payload.as_bytes());
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        parsed["prompt"],
        "中文记忆测试：行动完成 — Validação: não, memória, correção, ação"
    );
}

/// Runs a real Codex PowerShell peer with `payload` on stdin.
fn run_codex_script(script: &str, server: &str, payload: &str) -> std::process::Output {
    let script = repo_root().join("hooks").join("codex").join(script);
    let mut child = Command::new(ai_memory_test_support::powershell_exe())
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script)
        .env("AI_MEMORY_HOOK_URL", server)
        .env_remove("AI_MEMORY_CAPTURE_OWNER")
        .env_remove("AI_MEMORY_AUTH_TOKEN")
        .env_remove("AI_MEMORY_RUN_ID")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run Codex PowerShell hook");
    child
        .stdin
        .take()
        .expect("hook stdin")
        .write_all(payload.as_bytes())
        .expect("write hook payload");
    child
        .wait_with_output()
        .expect("wait for Codex PowerShell hook")
}

#[test]
fn codex_subagent_stop_never_posts_a_raw_assistant_message() {
    let invented = serde_json::json!({
        "session_id": "invented-parent",
        "agent_id": "invented-child",
        "agent_type": "worker",
    });

    // The raw assistant field must never leave the machine: no connection.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    let mut raw = invented.clone();
    raw["last_assistant_message"] = serde_json::json!("SENTINEL_CHILD_ASSISTANT");
    let output = run_codex_script("subagent-stop.ps1", &server, &raw.to_string());
    assert!(
        output.status.success(),
        "PowerShell hook failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"{}");
    // The POST is synchronous, so a connection would be queued by now.
    listener.set_nonblocking(true).unwrap();
    assert!(
        matches!(
            listener.accept(),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
        ),
        "a subagent-stop carrying last_assistant_message must not be posted"
    );

    // Control: the same script posts a payload without the field.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    let receiver = thread::spawn(move || receive_one_request(listener));
    let output = run_codex_script("subagent-stop.ps1", &server, &invented.to_string());
    assert!(output.status.success());
    assert_eq!(output.stdout, b"{}");
    let (headers, body) = receiver.join().unwrap();
    assert!(
        headers.contains("event=subagent-stop&agent=codex"),
        "{headers}"
    );
    assert_eq!(body, invented.to_string().as_bytes());
}
