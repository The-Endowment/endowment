#!/usr/bin/env bash
# Builds the program twice and runs every test.
#
# - `anchor build`: the deployable program and its IDL (target/deploy, target/idl).
# - a second build with `--features test-flagship` into target/deploy-test: the
#   same program with a fixed test flagship creator, so the integration tests
#   can create the flagship and exercise donations. Never deploy that build.
set -euo pipefail
cd "$(dirname "$0")/.."

# The test build first: `anchor build` compiles the tests too (for the IDL),
# and they embed it.
cargo build-sbf --manifest-path programs/endowment/Cargo.toml --features test-flagship --sbf-out-dir target/deploy-test
anchor build
cargo test -p endowment "$@"
