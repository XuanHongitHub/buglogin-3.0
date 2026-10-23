#!/usr/bin/env bash
set -euo pipefail

cargo fmt --all -- --check
node scripts/tauri-cargo.mjs check --workspace --all-targets
node scripts/tauri-cargo.mjs test --workspace
node scripts/tauri-cargo.mjs clippy --workspace --all-targets -- -D warnings
