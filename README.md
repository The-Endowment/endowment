# The $PENIS Endowment

Permanent capital. The only holder that can never pull out.

Website: [penis-endowment.vercel.app](https://penis-endowment.vercel.app) ([source](https://github.com/The-PENIS-Endowment/website))

The endowment is a Solana program that holds $PENIS forever and turns every PUMP it earns into more. Landlords (large holders) opt in by delegating their PUMP rewards to it, and nothing else.

## How it works

1. **Opt in.** A landlord signs one transaction: a standard token `Approve` on their PUMP account (and only that account), naming the endowment's authority PDA as delegate, plus `register_landlord`, which records the PUMP they already hold as their baseline and registers their $PENIS account.
2. **Activation.** Landlord sweeps run once registered landlords together hold 30% of $PENIS supply, and pause if that falls below 25%. Anyone can run the count once a day (`begin_count`, `count_landlords`, `finish_count`); balances are read directly from each landlord's registered $PENIS account.
3. **Sweep.** When a PUMP dividend drop lands, anyone can call `sweep`. It moves only the PUMP above the baseline into the endowment's PUMP vault and adds it to the landlord's on-chain contribution total.
4. **Buy.** Anyone can call `buyback`, at most once every 10 minutes. The contract decides the size (whatever the vault holds, within the per-trade and daily caps), swaps it for $PENIS in the Raydium PENIS/PUMP pool, and pays the caller a 0.25% tip in PUMP to cover network fees.
5. **Close at 200M.** Once the $PENIS vault holds 200,000,000 $PENIS, landlord contributions close for good. From then on, each buyback splits between buying $PENIS and adding permanent liquidity to the pool. The endowment's own dividends keep working forever.
6. **Opt out.** The landlord calls the token program's `Revoke`. It doesn't depend on this program, so it works even while the endowment is paused.

The endowment's own $PENIS earns PUMP dividends too. They land directly in the PUMP vault and are bought back like everything else.

## Guarantees in the code

- **No way out for $PENIS.** No instruction transfers tokens out of the $PENIS vault, and every buyback checks that the vault's balance never goes down.
- **Liquidity is permanent.** LP tokens land in an account owned by the endowment's authority. No instruction can withdraw them.
- **Landlord exposure is limited.** A landlord's only exposure is PUMP above their baseline in the one delegated account.
- **Buybacks are bounded.** PUMP can only go to the configured Raydium pool (plus the capped tip), and $PENIS can only land in the $PENIS vault. Each buy is capped per transaction and per 24 hours, and spaced at least 10 minutes apart. A fill is rejected if it is worse than `spot × (1 − 3% transfer fee) × (1 − pool fee − max price impact)`, with the price-impact allowance at 1% by default and a hard ceiling of 3%.
- **The count is honest.** Each landlord is counted at most once per round. Landlords who register mid-round sit it out; landlords who leave mid-round are removed from it, so a round can always be finished.
- **One-way switches.** Closing contributions (at 200M, or early by the admin's `retire`) and `renounce_admin` can never be undone, and neither can move funds.
- **Only the upgrade authority can initialize,** so nobody can front-run deployment.
- **The guardian can only pause.** A pause blocks sweeps and buybacks and expires on its own after 7 days. Only the admin can lift a pause early.
- **Every parameter has hard bounds.** The admin can tune buy limits, spacing (1 minute to 1 day), the tip (at most 0.5%), the post-close buy/liquidity split, and the activation thresholds (at most 50%), always within bounds fixed in code. The admin role can be handed over in two steps or renounced for good.

## PUMP's transfer hook

PUMP's mint has a Token-2022 transfer-hook extension whose program is currently unset. Every PUMP transfer this program makes itself (sweeps and tips) forwards the instruction's remaining accounts to Token-2022 when a hook is set, so those keep working if it is ever switched on: callers resolve the hook's extra accounts off-chain and pass them as remaining accounts. While no hook is set, remaining accounts are ignored. Swaps and deposits go through Raydium, which would need its own hook support.

## Status

Pre-launch, in testing.

| Scope | State |
|---|---|
| Opt-in, sweeps, pause, permanent vaults | ✅ Built and tested |
| Buybacks: contract-sized, spaced, tipped | ✅ Built and tested against mainnet pool state |
| Activation threshold, 200M close, post-close locked liquidity | ✅ Built and tested against mainnet pool state |
| Test period with small caps, then the upgrade key is destroyed | Planned |

## Development

Requires Rust, the Solana CLI, and Anchor 1.1.2 (`avm install 1.1.2`).

```sh
anchor build   # compile the program and generate the IDL
cargo test     # unit tests + LiteSVM integration tests against Token-2022 and Raydium CPMM
```

Layout:

```
programs/endowment/src/
  lib.rs            instruction entrypoints
  state.rs          Config and Landlord accounts, sweep math, activation hysteresis
  instructions/     initialize, register_landlord, deregister_landlord, sweep, buyback,
                    count (begin/count/finish), close_contributions, pause, roles
  raydium.rs        Raydium CPMM account views, swap and deposit (layouts from idls/raydium_cp_swap.json)
  transfer.rs       hook-aware PUMP transfers
  math.rs           buy sizing, post-close split, price floor, LP sizing, daily window
programs/endowment/tests/
  test_endowment.rs end-to-end tests: opt-in, activation, sweeps, buybacks, liquidity, roles
  fixtures/         mainnet snapshots of the Raydium CPMM program and the PENIS/PUMP pool
```

Follow [@PenisEndowment](https://x.com/PenisEndowment).
