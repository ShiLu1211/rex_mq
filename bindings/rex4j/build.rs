//! Build script for the `rex4j` interop test.
//!
//! Probes the local toolchain and emits:
//!   - HAS_JAVA (whether `java` was found on PATH)
//!   - REX4J_SO_PATH (absolute path to `librex4j.so`)
//!   - REX4J_TEST_CLASS_PATH (`target/release` for the example JNI .so lookup)

use std::path::PathBuf;

fn main() {
    let has_java = std::process::Command::new("java")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    println!("cargo:rustc-env=HAS_JAVA={}", has_java);

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let so_name = "librex4j.so";
    let target_debug = out_dir.ancestors().nth(3).unwrap_or(&out_dir);
    let target_release = target_debug
        .parent()
        .unwrap_or(target_debug)
        .join("release");
    let so_path = target_release.join(so_name);

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let classes_dir = manifest_dir.join("target").join("classes");

    // Compute the runtime classpath via `mvn dependency:build-classpath`
    // (uses the local m2 cache, no network). If that fails (no mvn, or
    // deps not downloaded), fall back to a classpath that includes
    // only the local classes — the test will then fail with a clear
    // error about the missing m2 cache.
    let mvn_cp = std::process::Command::new("mvn")
        .arg("-f")
        .arg(manifest_dir.join("pom.xml"))
        .arg("-q")
        .arg("dependency:build-classpath")
        .arg("-Dmdep.outputFile=/tmp/rex4j-cp.txt")
        .arg("-Dmdep.includeScope=runtime")
        .output();
    let mut classpath = format!("{}:{}", target_release.display(), classes_dir.display());
    if let Ok(out) = mvn_cp {
        if out.status.success() {
            if let Ok(content) = std::fs::read_to_string("/tmp/rex4j-cp.txt") {
                classpath = format!(
                    "{}:{}:{}",
                    content.trim(),
                    target_release.display(),
                    classes_dir.display()
                );
            }
        }
    }

    println!(
        "cargo:rustc-env=REX4J_SO_PATH={}",
        so_path.to_string_lossy()
    );
    println!("cargo:rustc-env=REX4J_TEST_CLASS_PATH={}", classpath);
    println!(
        "cargo:rustc-env=REX4J_CLASSES_DIR={}",
        classes_dir.display()
    );

    println!("cargo:rerun-if-env-changed=REX4J_SKIP_INTEROP");
}
