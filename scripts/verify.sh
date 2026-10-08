#!/usr/bin/env bash
set -euo pipefail

cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test

# read-only invariant, compile-time flavour
! grep -rn "\.put(\|\.patch(\|\.delete(\|Method::PUT\|Method::PATCH\|Method::DELETE" src/

# no leftovers
! grep -rn "todo!()\|unimplemented!()" src/

cargo build --release
ls -la target/release/issuemesh

SECRETS="${ISSUEMESH_HOME:-$HOME/.issuemesh}/secrets.env"
if [ -f "$SECRETS" ]; then
  ISSUEMESH_E2E=1 cargo test --release --test e2e -- --ignored --test-threads=1
  if command -v node >/dev/null 2>&1; then
    node scripts/browser_e2e.mjs
  fi
else
  echo "secrets.env not found: live e2e suite skipped"
fi
