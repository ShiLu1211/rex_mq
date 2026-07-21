//! Tiny build-time helpers for `tests/interop.rs`.

#![cfg(test)]

pub fn has_java_runtime() -> bool {
    option_env!("HAS_JAVA") == Some("true")
}

pub fn rex4j_so_path() -> std::path::PathBuf {
    std::path::PathBuf::from(option_env!("REX4J_SO_PATH").unwrap_or(""))
}

pub fn rex4j_test_class_path() -> String {
    option_env!("REX4J_TEST_CLASS_PATH")
        .unwrap_or("")
        .to_string()
}
