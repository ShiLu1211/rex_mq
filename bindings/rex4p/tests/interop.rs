//! Cross-language interop test for the Python binding.
//!
//! Per `docs/superpowers/specs/2026-07-20-bindings-cross-language-interop-test-design.md`
//! (PR 2-6 in the original plan), this test:
//!
//! 1. Starts a `rex-server` on an ephemeral TCP port.
//! 2. Builds the `librex4p.so` cdylib (already done by cargo).
//! 3. Spawns a Python subprocess that imports `rex4p`, subscribes to
//!    a unique title, and echoes every payload byte to stdout.
//! 4. Publishes once from a Rust `rex-client`.
//! 5. Reads the subprocess stdout and asserts the payload appears
//!    within a timeout. This proves the protocol round-trips
//!    through three independent implementations (rex-server wire
//!    encoder, rex4p decoder, Python handler).
//!
//! Gated on `#[cfg(has_python3)]` — build.rs probes `python3 --version`
//! and emits this cfg flag. On a machine without Python the test
//! simply doesn't exist (vs. a runtime panic that's harder to triage).
//!
//! This test only runs against TCP because that's the path Python's
//! rex4p binding is exercised through (rex4p.so links librex4j.so's
//! wire format but runs on the host's libc, so QUIC/WSS would need
//! separate work).

#![cfg(has_python3)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rex_client::{RexClientConfig, RexClientHandlerTrait, open_client};
use rex_core::{Protocol, RexClientInner, RexCommand, RexData};
use rex_test::factory::TestEnv;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use uuid::Uuid;

/// Inline Python driver that:
///  - imports rex4p
///  - connects to the host:port
///  - logs into the title
///  - for each incoming message, prints the payload bytes escaped as
///    a hex string with a known prefix so the Rust test can grep
///
/// The hex-printing is intentional: it sidesteps the spec's "verbatim
/// payload in stdout" assumption, which doesn't hold for `rex_engine.py`
/// (the demo prints only latency stats). Our driver echoes payload so
/// the assertion is unambiguous.
const PY_DRIVER: &str = r#"
import sys
import time
from rex4p import ClientConfig, RexClient, Protocol, RexCommand, RexData

class Handler:
    def on_login(self, data):
        print("LOGIN_OK:" + str(data), flush=True)

    def on_message(self, data):
        # Hex-encode the payload bytes so the Rust test can grep for
        # an unambiguous sentinel. data.data is a Python `bytes`
        # object (the PyRexData getter exposes Vec<u8> as bytes).
        payload = bytes(data.data)
        print("PAYLOAD_HEX:" + payload.hex(), flush=True)

host, port, title = sys.argv[1], sys.argv[2], sys.argv[3]

handler = Handler()
config = ClientConfig(f"{host}:{port}", Protocol.tcp(), title, handler)
client = RexClient()
client.connect(config)

# Wait for the login handshake to complete. The Rust publisher will
# be racing to send its payload, so this must finish before the
# publisher's send is queued (otherwise the message lands before the
# subscriber is registered and the server drops it).
deadline = time.time() + 5.0
while not client.is_connected() and time.time() < deadline:
    time.sleep(0.05)

if not client.is_connected():
    print("CONNECT_FAILED", flush=True)
    sys.exit(1)

# Idle forever. on_message fires from the binding's background
# thread whenever the server forwards a message; the Rust test
# will kill us via SIGKILL once it has seen PAYLOAD_HEX. We block
# on a sleep loop rather than consume stdin so the test never has
# to coordinate a "GO" handshake — closing our stdin would cause
# us to exit and the next message would land in a dead process.
while True:
    time.sleep(1.0)
"#;

#[tokio::test(flavor = "current_thread")]
async fn python_binding_receives_payload_published_by_rust_client() -> Result<()> {
    // 1. Bring up a server on an ephemeral TCP port.
    let mut env = TestEnv::new().await;
    let server = env
        .start_server(Protocol::Tcp)
        .await
        .context("start server")?;
    let server_addr: SocketAddr = server.addr();
    let (host, port) = (server_addr.ip().to_string(), server_addr.port());

    // 2. Unique title + payload (UUID makes parallel CI runs safe).
    let title = format!("interop-{}", Uuid::new_v4());
    let payload = format!("interop-payload-{}", Uuid::new_v4()).into_bytes();

    // 3. Resolve the cdylib path. cargo puts it at
    //    `<workspace>/target/<profile>/librex4p.so`. We must compute
    //    the absolute path: `cargo test` runs the test binary with
    //    cwd = `target/<profile>/deps/`, so a relative `target/...`
    //    path resolves to a non-existent nested directory. Use
    //    `CARGO_TARGET_DIR` (set by cargo, always absolute) when
    //    available, otherwise resolve via `CARGO_MANIFEST_DIR`
    //    (also absolute, set at compile time).
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let target_dir = std::env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| {
        // CARGO_MANIFEST_DIR is `<workspace>/bindings/rex4p`; the
        // workspace target dir is `<workspace>/target`.
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        manifest
            .parent() // bindings/
            .and_then(|p| p.parent()) // workspace root
            .expect("rex4p manifest dir should be at <workspace>/bindings/rex4p")
            .join("target")
            .to_string_lossy()
            .into_owned()
    });
    let cdylib_dir = PathBuf::from(target_dir).join(&profile);
    let cdylib_name = if cfg!(target_os = "macos") {
        "librex4p.dylib"
    } else {
        "librex4p.so"
    };
    let cdylib_path = cdylib_dir.join(cdylib_name);
    if !cdylib_path.exists() {
        // `crate-type = ["cdylib"]` only emits the .so on explicit
        // `cargo build -p rex4p`; `cargo test` alone doesn't trigger
        // it. Self-heal by invoking cargo from inside the test so a
        // plain `cargo test -p rex4p --test interop` works in any
        // environment. CI is fixed either way (see .github/workflows).
        eprintln!(
            "rex4p cdylib not found at {}; running `cargo build -p rex4p` to build it",
            cdylib_path.display()
        );
        let status = std::process::Command::new("cargo")
            .args(["build", "-p", "rex4p"])
            .status()
            .context("spawn cargo build -p rex4p")?;
        if !status.success() {
            bail!(
                "cargo build -p rex4p failed (status {status:?}); \
                 cdylib still missing at {}",
                cdylib_path.display()
            );
        }
        if !cdylib_path.exists() {
            bail!(
                "cargo build -p rex4p succeeded but cdylib still \
                 missing at {}",
                cdylib_path.display()
            );
        }
    }

    // Python's import machinery looks for `rex4p.so` (no `lib`
    // prefix) but cargo's cdylib output is `librex4p.so` on Linux.
    // Create a hardlink with the expected name so `import rex4p`
    // succeeds. The hardlink shares the inode (no extra disk
    // space) and persists for the rest of the test run; cargo's
    // next rebuild will replace the original .so in place and the
    // hardlink continues to point at the new content. (This is the
    // same workaround documented in README for the rex_engine.py
    // example.)
    let py_module_path = cdylib_dir.join(if cfg!(target_os = "macos") {
        "rex4p.dylib"
    } else {
        "rex4p.so"
    });
    if !py_module_path.exists() {
        std::fs::hard_link(&cdylib_path, &py_module_path)
            .context("hardlink rex4p.so for python import")?;
    }

    // 4. Spawn the Python driver. PYTHONPATH = cdylib_dir so `import rex4p`
    //    finds the just-built .so. We pass -- so flags after the script
    //    name don't get parsed by rex4p (defensive).
    let script = tempfile_py_driver().context("write python driver")?;
    let mut child = Command::new("python3")
        .arg("-u") // unbuffered — payloads must hit stdout immediately
        .arg(&script)
        .arg(&host)
        .arg(port.to_string())
        .arg(title.clone())
        .env("PYTHONPATH", &cdylib_dir)
        // Inherit stdin (closed) rather than piping it. A piped
        // stdin that the parent never writes to causes the Python
        // driver to see EOF on stdin and exit — which would kill
        // the subscriber before the Rust publisher sends. Letting
        // stdin be inherited from the parent (also closed in CI)
        // puts the Python driver into the same EOF state, but it
        // never reads stdin in the idle loop, so this is a no-op.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn python3")?;

    // Wait for LOGIN_OK. The driver prints it as soon as the
    // login_ok callback fires (background thread inside the
    // binding); the Rust publisher cannot send before this point or
    // the server drops the message because the subscriber isn't
    // registered yet.
    let stdout = child.stdout.take().expect("stdout piped");
    let mut reader = BufReader::new(stdout).lines();
    let logged_in_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut logged_in = false;
    while tokio::time::Instant::now() < logged_in_deadline {
        let line = match tokio::time::timeout(Duration::from_secs(1), reader.next_line()).await {
            Ok(Ok(Some(l))) => l,
            Ok(Ok(None)) => break,
            Ok(Err(e)) => bail!("read python stdout: {e}"),
            Err(_) => continue,
        };
        if line.starts_with("LOGIN_OK") {
            logged_in = true;
            break;
        }
        if line == "CONNECT_FAILED" {
            bail!("python client failed to connect within 5s");
        }
    }
    if !logged_in {
        bail!("python client never logged in within 5s");
    }

    // 5. Open a Rust publisher and send one Title message.
    let publisher = open_client(RexClientConfig::new(
        Protocol::Tcp,
        server_addr,
        &title,
        Arc::new(NoopClientHandler),
    ))
    .await
    .context("open publisher")?;
    let mut rex_data = RexData::new(RexCommand::Title, &title, payload.as_slice());
    publisher
        .send_data(&mut rex_data)
        .await
        .context("send_data")?;

    // 6. Drain stdout, looking for our payload. The hex prefix keeps
    //    the search unambiguous if the demo ever evolves.
    let expected_hex = format!(
        "PAYLOAD_HEX:{}",
        payload
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let mut found = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let line = match tokio::time::timeout(remaining, reader.next_line()).await {
            Ok(Ok(Some(l))) => l,
            Ok(Ok(None)) => break,
            Ok(Err(e)) => bail!("read python stdout: {e}"),
            Err(_) => break,
        };
        if line.contains(&expected_hex) {
            found = true;
            break;
        }
    }

    // 7. Cleanup — kill the python process unconditionally; surface
    //    any exit-code signal as a diagnostic even on success.
    let _ = child.kill().await;
    let status = child.wait().await.context("wait python")?;
    if !found {
        bail!(
            "payload {expected_hex:?} not in python stdout within 10s; \
             python exit status: {status:?}"
        );
    }
    Ok(())
}

/// Write the Python driver to a temp file. We use a temp file (not
/// stdin) so the script appears in `ps` with its name when CI is
/// stuck — much easier to debug than `python3 -c '...'` from a Rust
/// arg vector.
fn tempfile_py_driver() -> Result<PathBuf> {
    use std::io::Write;
    let dir = tempfile::tempdir().context("tempdir")?;
    let path = dir.path().join("rex4p_interop_driver.py");
    let mut f = std::fs::File::create(&path).context("create driver file")?;
    f.write_all(PY_DRIVER.as_bytes()).context("write driver")?;
    // Keep the TempDir alive by leaking it; the process exits shortly
    // anyway. Tests are short-lived; no cleanup-on-panic complexity
    // is worth introducing for a CI artifact.
    std::mem::forget(dir);
    Ok(path)
}

/// No-op handler for the Rust-side publisher. We only send; we don't
/// care about receiving acks or messages here. The Python subprocess
/// is the side under test for receiving.
struct NoopClientHandler;

#[async_trait::async_trait]
impl RexClientHandlerTrait for NoopClientHandler {
    async fn login_ok(&self, _client: Arc<RexClientInner>, _data: RexData) -> Result<()> {
        Ok(())
    }
    async fn handle(&self, _client: Arc<RexClientInner>, _data: RexData) -> Result<()> {
        Ok(())
    }
}
