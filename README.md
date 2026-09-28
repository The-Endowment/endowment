# The $PENIS Endowment

Permanent capital for buybacks, liquidity, and growth. The only holder that can never pull out.

The endowment is a Solana program that holds $PENIS forever and spends only the PUMP it earns. Landlords (large holders) opt in by delegating their PUMP rewards to it, and nothing else.

## How it works

1. **Opt in.** A landlord signs one transaction. It holds a standard token `Approve` on their PUMP account (and only that account), naming the endowment's authority PDA as delegate, plus `register_landlord`, which records the PUMP they already hold as their baseline.
2. **Sweep.** When a PUMP dividend drop lands, anyone can call `sweep`. It moves only the PUMP above the baseline into the endowment's PUMP vault and adds it to the landlord's on-chain contribution total.
3. **Opt out.** The landlord calls the token program's `Revoke`. It doesn't depend on this program, so it works even while the endowment is paused.

The endowment's own $PENIS earns PUMP dividends too. They land directly in the PUMP vault.

## Guarantees in the code

- **No way out for $PENIS.** No instruction transfers tokens out of the $PENIS vault.
- **Landlord exposure is limited.** A landlord's only exposure is PUMP above their baseline in the one delegated account.
- **Only the upgrade authority can initialize,** so nobody can front-run deployment.
- **The guardian can only pause.** A pause blocks sweeps and expires on its own after 7 days. The guardian can't move funds, and only the admin can lift a pause early.
- **Buybacks are bounded.** Anyone can trigger a buyback, but PUMP can only go to the configured Raydium PENIS/PUMP pool and the $PENIS can only land in the $PENIS vault. Each buy is capped per transaction and per 24 hours. A fill is rejected if it is worse than `spot × (1 − 3% transfer fee) × (1 − pool fee − max price impact)`, where the price-impact allowance defaults to 1% with a hard ceiling of 3%.
- **Keys can be rotated.** The admin can replace the guardian. The admin role changes hands in two steps, and the new key must sign to accept. Both roles are meant to be Squads multisigs.

## Status

| Phase | Scope | State |
|---|---|---|
| 1 | Opt-in, sweeps, pause, permanent vaults | ✅ Built and tested locally |
| 1 | Buybacks (PUMP → $PENIS in the Raydium PENIS/PUMP pool) | ✅ Built and tested against mainnet pool state |
| 2 | Web app: delegate/revoke, dashboard, leaderboard, ledger | Planned |
| 3 | Thermostat: liquidity adds, marketing stream, timelocked parameters | Planned (after an external audit) |

Not deployed to mainnet. Not audited.

## Development

Requires Rust, the Solana CLI, and Anchor 1.1.2 (`avm install 1.1.2`).

```sh
anchor build   # compile the program and generate the IDL
cargo test     # unit tests + LiteSVM integration tests against Token-2022
```

Layout:

```
programs/endowment/src/
  lib.rs            instruction entrypoints
  state.rs          Config and Landlord accounts, sweep math
  instructions/     initialize, register_landlord, sweep, deregister_landlord, pause, roles, buyback
  raydium.rs        Raydium CPMM account views and swap CPI (layouts from idls/raydium_cp_swap.json)
  math.rs           buyback price floor and daily window
programs/endowment/tests/
  test_endowment.rs end-to-end tests: opt-in, sweeps, revoke, pause, roles, buybacks
  fixtures/         mainnet snapshots of the Raydium CPMM program and the PENIS/PUMP pool
```

Follow [@PenisEndowment](https://x.com/PenisEndowment).
