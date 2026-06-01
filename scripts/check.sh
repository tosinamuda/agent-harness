#!/usr/bin/env bash
# Local quality gate — run before pushing. The free stand-in for hosted CI:
# the same checks a CI workflow would run, on your machine, no service needed.
#
#   ./scripts/check.sh
set -euo pipefail
cd "$(dirname "$0")/.."

# Note: this codebase uses deliberate compact hand-formatting, so `cargo fmt
# --check` is intentionally NOT a gate here — it would rewrite the whole tree.
# clippy below is the lint gate that's actually enforced.

echo "==> clippy (all targets, all features, warnings = errors)"
cargo clippy --workspace --all-targets --all-features -- -D warnings

echo "==> tests"
cargo test --workspace

echo "==> build all targets (incl. examples)"
cargo build --workspace --all-targets

echo "==> feature gates (lean framework, then claude-only)"
cargo build -p agent-harness --no-default-features
cargo build -p agent-harness --no-default-features --features claude

if command -v cargo-deny >/dev/null 2>&1; then
  echo "==> cargo deny (advisories + licenses)"
  cargo deny check
else
  echo "==> (skip) cargo-deny not installed — 'cargo install cargo-deny' to enable"
fi

echo
echo "All checks passed."
