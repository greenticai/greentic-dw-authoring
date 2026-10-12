#!/usr/bin/env bash
set -euo pipefail

# The single definition of "green" for this repo: `ci.yml` runs exactly this
# script, so a local pass and a CI pass mean the same thing. Keep them that way
# — a check that lives only in the workflow cannot be run before pushing.

echo "==> cargo fmt"
cargo fmt --all -- --check

echo "==> cargo clippy"
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

echo "==> cargo test"
cargo test --workspace --all-features --locked

echo
echo "All checks passed."
