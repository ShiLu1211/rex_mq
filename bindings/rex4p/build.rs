//! Build script for `rex4p` — probes for `python3` at compile time
//! and emits a `has_python3` cfg flag for the integration test
//! (`tests/interop.rs`) to gate on. This lets `cargo test -p rex4p`
//! skip cleanly on machines that don't have Python available, instead
//! of failing at runtime with a confusing "No such file" error.
//!
//! Probe is intentionally `which python3` rather than a deeper
//! import test: pyo3 already validated the Python headers at cdylib
//! compile time (otherwise the crate would not have built at all).

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-check-cfg=cfg(has_python3)");

    let has_python3 = Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if has_python3 {
        println!("cargo:rustc-cfg=has_python3");
    } else {
        println!(
            "cargo:warning=python3 not found on PATH; \
             rex4p/tests/interop.rs will be skipped at compile time"
        );
    }
}
