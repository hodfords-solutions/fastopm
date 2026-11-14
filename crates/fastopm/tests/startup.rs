//! Runs the real binary: startup, a request over TCP and over a unix socket, graceful stop.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_fastopm");

fn write_config(dir: &Path, extra: &str) -> std::path::PathBuf {
    let path = dir.join("config.yaml");
    let yaml = format!(
        "storage: ./storage\nuplinks:\n  npmjs:\n    url: http://127.0.0.1:9/\npackages:\n  \
         '**':\n    access: $all\n    proxy: npmjs\nlog:\n  type: stdout\n  level: http\n{extra}"
    );
    std::fs::write(&path, yaml).unwrap();
    path
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn ping_request() -> &'static [u8] {
    b"GET /-/ping HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
}

fn exchange(mut stream: impl Read + Write) -> String {
    stream.write_all(ping_request()).unwrap();
    let mut out = String::new();
    let _ = stream.read_to_string(&mut out);
    out
}

fn wait_for(mut attempt: impl FnMut() -> Option<String>) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(out) = attempt() {
            return out;
        }
        assert!(Instant::now() < deadline, "server did not answer in time");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn stop(mut child: Child) -> String {
    Command::new("kill").arg("-TERM").arg(child.id().to_string()).status().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "server did not stop on SIGTERM");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "exit status {status}");
    let mut log = String::new();
    child.stdout.take().unwrap().read_to_string(&mut log).unwrap();
    log
}

#[test]
fn serves_ping_over_tcp_and_stops_on_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), "web:\n  enable: false\n");
    let port = free_port();
    let child = Command::new(BIN)
        .args(["--config", config.to_str().unwrap(), "--listen", &format!("127.0.0.1:{port}")])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .env_remove("RUST_LOG")
        .spawn()
        .unwrap();
    let response = wait_for(|| {
        let stream = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
        Some(exchange(stream))
    });
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("\r\n\r\n{}\n"), "{response}");
    let log = stop(child);
    assert!(log.contains("http address"), "{log}");
    assert!(log.contains("`web` is not supported"), "{log}");
}

#[cfg(unix)]
#[test]
fn serves_over_a_unix_socket() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), "");
    let socket = dir.path().join("fastopm.sock");
    let child = Command::new(BIN)
        .args(["--config", config.to_str().unwrap(), "--listen"])
        .arg(format!("unix:{}", socket.display()))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let response = wait_for(|| {
        let stream = std::os::unix::net::UnixStream::connect(&socket).ok()?;
        Some(exchange(stream))
    });
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    stop(child);
    assert!(!socket.exists(), "socket file is removed on shutdown");
}

#[test]
fn refuses_https_listen() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(dir.path(), "");
    let output = Command::new(BIN)
        .args(["--config", config.to_str().unwrap(), "--listen", "https://localhost:4873"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("TLS"));
}

#[test]
fn reports_a_bad_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    std::fs::write(&path, "uplinks:\n  all:\n    url: http://x/\n").unwrap();
    let output = Command::new(BIN).args(["--config", path.to_str().unwrap()]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).starts_with("fastopm: "));
}
