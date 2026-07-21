//! Cross-language interop test for `rex4j`.
//!
//! Boots `rex-server`, spawns `examples/RexEngine.java` as a child
//! JVM process in `rcv` mode, publishes one `RexData::Title` from
//! a Rust client, and asserts the foreign-side handler prints TPS
//! stats (the java example runs in bench mode and prints `tps: N`
//! every second after receiving data).

use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use rand::fill;
use rex_core::Protocol;
use rex_test::factory::TestEnv;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use uuid::Uuid;

#[path = "../tests_build_helpers.rs"]
mod tests_build_helpers;

const REX4J_SKIP_INTEROP: Option<&'static str> = option_env!("REX4J_SKIP_INTEROP");

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interop_smoke() -> Result<()> {
    if REX4J_SKIP_INTEROP == Some("1") {
        eprintln!("REX4J_SKIP_INTEROP=1 set, skipping");
        return Ok(());
    }
    if !tests_build_helpers::has_java_runtime() {
        eprintln!("java not available, skipping rex4j interop test");
        return Ok(());
    }

    let so_path = tests_build_helpers::rex4j_so_path();
    anyhow::ensure!(
        so_path.exists(),
        "librex4j.so not built; run `cargo build --release -p rex4j` first (looked at {})",
        so_path.display()
    );

    // Unique per run so parallel CI tests don't collide.
    let run_id = Uuid::new_v4();
    let title = format!("rex4j-interop-{}", run_id);
    let mut payload = [0u8; 64];
    fill(&mut payload[..]);

    // 1. Boot the server in-process via the existing rex-test helper.
    let mut env = TestEnv::new().await;
    let server = env
        .start_server(Protocol::Tcp)
        .await
        .context("starting rex-server")?;
    let addr = server.addr();
    eprintln!("rex-server listening on {}", addr);

    // 2. Spawn the foreign-side Java demo. The example program reads
    //    `-h` / `-p` / `-t` / `-y` (lowercase h, different from python)
    //    and prints TPS stats every 1s after receiving data.
    let classpath = tests_build_helpers::rex4j_test_class_path();
    // Pre-flight: the .class files must exist (require `mvn compile`
    // first). Otherwise fail with a clear message instead of waiting
    // 10s for a ClassNotFoundException.
    let classes_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("classes")
        .join("com")
        .join("rex4j")
        .join("example")
        .join("RexEngine.class");
    anyhow::ensure!(
        classes_dir.exists(),
        "RexEngine.class not built; run `mvn compile` in bindings/rex4j first (looked at {})",
        classes_dir.display()
    );

    let mut child = Command::new("java");
    child
        .arg(format!(
            "-Djava.library.path={}/release",
            classpath.split(':').next().unwrap_or("")
        ))
        .arg("-cp")
        .arg(&classpath)
        .arg("com.rex4j.example.RexEngine")
        .arg("-h")
        .arg(addr.ip().to_string())
        .arg("-p")
        .arg(addr.port().to_string())
        .arg("-t")
        .arg(&title)
        .arg("-y")
        .arg("rcv")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = child
        .spawn()
        .with_context(|| "spawning java -cp ... RexEngine")?;

    // 3. Drain the child's stdout into a shared buffer.
    let mut stdout = child.stdout.take().expect("piped stdout");
    let captured: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = Arc::clone(&captured);
    let drain_handle = tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            match stdout.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if let Ok(mut cap) = captured_clone.lock() {
                        cap.extend_from_slice(&buf[..n]);
                    }
                }
                Err(_) => break,
            }
        }
    });

    // 4. Give the JVM ~2s to bind the library and subscribe via rex-client.
    tokio::time::sleep(Duration::from_millis(2000)).await;

    // 5. Publish one RexData from a Rust TestClient.
    //    The java example's rcv handler runs in bench mode — it
    //    expects a long timestamp prefix (little-endian) and prints
    //    TPS every 1s. Prepend now_nanos() to satisfy the handler.
    let publisher = env
        .create_client_to_addr(addr, &title)
        .await
        .context("creating publisher client")?;
    let now_ns = rex_core::utils::now_micros() * 1_000; // micros → nanos
    let mut bench_payload = Vec::with_capacity(8 + payload.len());
    bench_payload.extend_from_slice(&now_ns.to_le_bytes());
    bench_payload.extend_from_slice(&payload);
    publisher
        .send(rex_core::RexCommand::Title, &title, &bench_payload)
        .await
        .context("publishing payload")?;
    drop(publisher);

    // 6. Wait up to 10s for the JVM to print tps stats.
    let deadline = tokio::time::sleep(Duration::from_secs(10));
    tokio::pin!(deadline);
    let mut found = false;
    loop {
        {
            let cap = captured.lock().unwrap();
            if cap.windows(4).any(|w| w == b"tps:") {
                found = true;
            }
        }
        if found {
            break;
        }
        tokio::select! {
            _ = &mut deadline => break,
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }

    drop(drain_handle);
    let _ = child.kill().await;

    if !found {
        let cap = captured.lock().unwrap();
        let stdout_str = String::from_utf8_lossy(&cap);
        let exit_status = child.wait().await.ok().and_then(|s| s.code());
        let mut stderr_buf = Vec::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut stderr_buf).await;
        }
        let stderr_str = String::from_utf8_lossy(&stderr_buf);
        anyhow::bail!(
            "timed out after 10s waiting for java child to print tps stats (exit_status={:?}, stdout: {}, stderr: {})",
            exit_status,
            &stdout_str[..stdout_str.len().min(4096)],
            &stderr_str[..stderr_str.len().min(4096)]
        );
    }

    Ok(())
}
