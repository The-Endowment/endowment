#!/usr/bin/env bash
# Builds the program twice and runs every test.
#
# - `anchor build`: the deployable program and its IDL (target/deploy, target/idl).
# - a second build with `--features test-flagship` into target/deploy-test: the
#   same program with a fixed test flagship creator, so the integration tests
#   can create the flagship and exercise donations. Never deploy that build.
#
# Both builds are checked for SBF stack-frame overflows. cargo-build-sbf prints
# these as "Error: ... Stack offset ... exceeded" but still writes the .so, and
# such a program can corrupt its own stack at runtime, so we fail the script.
set -euo pipefail
cd "$(dirname "$0")/.."

log="$(mktemp)"
trap 'rm -f "$log"' EXIT

# The test build first: `anchor build` compiles the tests too (for the IDL),
# and they embed it.
cargo build-sbf --manifest-path programs/endowment/Cargo.toml --features test-flagship --sbf-out-dir target/deploy-test 2>&1 | tee "$log"
anchor build 2>&1 | tee -a "$log"

if grep -E "exceeded max offset|overwrites values in the frame" "$log"; then
  echo "SBF stack overflow reported by the build; refusing to continue." >&2
  exit 1
fi

cargo test -p endowment "$@"
