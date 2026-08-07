#!/usr/bin/env bash
# Manual end-to-end smoke for the cross-language bindings.
#
# Builds both binding cdylibs and runs each crate's `tests/interop.rs`
# under the cargo test runner. Used as a local sanity check before
# pushing to CI — CI runs the same suites via the rust-integration
# job in `.github/workflows/rust.yml`.
#
# Usage:   ./scripts/interop.sh
# Exits 0 only if every binding's interop test passes.

set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> building binding cdylibs"
cargo build -p rex4p -p rex4j

echo "==> running rex4p interop"
cargo test -p rex4p --test interop -- --nocapture

echo "==> running rex4j interop"
cargo test -p rex4j --test interop -- --nocapture

echo "✅ all bindings received the same payload"
