# The $PENIS Endowment

Permanent capital. The only holder that can never pull out.

Website: [penis-endowment.vercel.app](https://penis-endowment.vercel.app) ([source](https://github.com/The-PENIS-Endowment/website))

This is a Solana program for **endowments**: vaults that hold a dividend-paying meme coin forever and turn every dividend they receive into more of it. The largest holders ("landlords") opt in by delegating their dividend rewards, and nothing else.

**One shared contract, many endowments.** Anyone can create an endowment for their own coin on this same deployed, audited program. $PENIS (paid in PUMP) is the flagship. Every endowment gets the same guarantees, enforced by the same code, and each one is fully isolated from the others.

## How it works

1. **Create.** Anyone calls `create_endowment` for a coin, its dividend asset, and the Raydium CPMM pool that trades one against the other. The program checks that the pool holds exactly those two mints, and that every parameter is within hard bounds. The creator becomes the admin unless another admin is named.
2. **Opt in.** A landlord signs one transaction: a standard token `Approve` on their dividend account (and only that account), naming the endowment's authority PDA as delegate, plus `register_landlord`, which records the dividend they already hold as their baseline and registers their coin account.
3. **Activation.** Landlord sweeps run once registered landlords together hold the activation share of the coin's supply (30% by default), and pause if that falls below the deactivation share (25% by default). Anyone can run the count once a day (`begin_count`, `count_landlords`, `finish_count`); balances are read directly from each landlord's registered coin account.
4. **Sweep.** When a dividend drop lands, anyone can call `sweep`. It moves only the dividend above the baseline into the endowment's dividend vault and adds it to the landlord's on-chain contribution total.
5. **Buy.** Anyone can call `buyback`, at most once per interval (10 minutes by default). The contract decides the size (whatever the vault holds, within the per-trade and daily caps), swaps it for the coin in the endowment's pool, and pays the caller a small tip to cover network fees.
6. **Close at the cap.** Once the coin vault holds the contribution cap (200,000,000 $PENIS for the flagship), landlord contributions close for good. From then on, each buyback splits between buying the coin and adding permanent liquidity to the pool. The endowment's own dividends keep working forever.
7. **Opt out.** The landlord calls the token program's `Revoke`. It doesn't depend on this program, so it works even while the endowment is paused.

An endowment's own coin earns dividends too. They land directly in its dividend vault and are bought back like everything else.

## Guarantees in the code

- **No way out for the coin.** No instruction transfers tokens out of any endowment's coin vault, and every buyback checks that the vault's balance never goes down.
- **Liquidity is permanent.** LP tokens land in an account owned by the endowment's authority. No instruction can withdraw them.
- **Endowments are isolated.** Every endowment's config, authority, vaults and landlords are derived from its own address, and every account an instruction touches is derived from, or checked against, that endowment's config. One endowment's cranks can't read or move another's funds, landlords or counts.
- **Nobody can squat an endowment.** An endowment's address includes its creator, so anyone else "creating" it only ever creates a separate endowment of their own. Pre-creating its vault token accounts can't block creation either.
- **Landlord exposure is limited.** A landlord's only exposure is dividend above their baseline in the one delegated account.
- **Buybacks are bounded.** The dividend can only go to the endowment's Raydium pool, the capped tip, or the donation locked in at creation, and the coin can only land in the endowment's coin vault. Each buy is capped per transaction and per 24 hours, and spaced out. A fill is rejected if it is worse than `spot × (1 − coin transfer fee) × (1 − pool fee − max price impact)`, with the price-impact allowance between 0.1% and 3%.
- **The count is honest.** Each landlord is counted at most once per round, only in its own endowment's count. Landlords who register mid-round sit it out; landlords who leave mid-round are removed from it, so a round can always be finished.
- **One-way switches.** Closing contributions (at the cap, or early by the admin's `retire`) and `renounce_admin` can never be undone, and neither can move funds.
- **The guardian can only pause.** A pause blocks sweeps and buybacks and expires on its own after 7 days. Only the admin can lift a pause early.
- **Every parameter has hard bounds.** The admin can tune buy limits, spacing (1 minute to 1 day), the tip (at most 0.5%), the post-close buy/liquidity split, and the activation thresholds (at most 50%), always within bounds fixed in code. The admin role can be handed over in two steps or renounced for good.

## The optional donation

When creating an endowment, a project can choose to donate **0%, 0.1%, 0.2% or 0.3%** of every buyback to the flagship $PENIS endowment, as thanks for the shared contract, website, automation and audit work. The rate is locked at creation and can never change. The donation is paid in the dividend asset and lands directly in the flagship endowment's dividend vault, where it is bought back into $PENIS like everything else. Nobody holds it.

Donations are only possible for coins that pay in the flagship's dividend asset (PUMP). The tip and the donation together are capped at 0.8% of each buy.

The flagship endowment's address is a constant in the program (`FLAGSHIP_CONFIG`). **It is a placeholder until the $PENIS endowment is created on mainnet**, and must be set to that endowment's config address, and the program redeployed, before the upgrade authority is burned.

## Use it, or run your own

- **Use the shared contract (recommended).** Create your endowment on this deployed program. You inherit its audit, its track record, the shared automation, and the explorer.
- **Deploy your own copy.** The code is open source under the Apache-2.0 license. You can deploy it yourself, but your copy won't share this contract's audit or track record.

## Transfer hooks

A Token-2022 dividend mint may carry a transfer-hook extension (PUMP's is currently unset). Every dividend transfer this program makes itself (sweeps, tips and donations) forwards the instruction's remaining accounts to the token program when a hook is set, so those keep working if it is ever switched on: callers resolve the hook's extra accounts off-chain and pass them as remaining accounts. While no hook is set, remaining accounts are ignored. Swaps and deposits go through Raydium, which would need its own hook support.

## Status

Pre-launch, in testing.

| Scope | State |
|---|---|
| Opt-in, sweeps, pause, permanent vaults | ✅ Built and tested |
| Buybacks: contract-sized, spaced, tipped | ✅ Built and tested against mainnet pool state |
| Activation threshold, contribution cap, post-close locked liquidity | ✅ Built and tested against mainnet pool state |
| Many endowments on one contract, optional flagship donation | ✅ Built and tested |
| Test period with small caps, then the upgrade key is destroyed | Planned |

## Development

Requires Rust, the Solana CLI, and Anchor 1.1.2 (`avm install 1.1.2`).

```sh
anchor build   # compile the program and generate the IDL
cargo test     # unit tests + LiteSVM integration tests against Token-2022, SPL Token and Raydium CPMM
```

Layout:

```
programs/endowment/src/
  lib.rs            instruction entrypoints
  state.rs          Config and Landlord accounts, creation params, sweep math, activation hysteresis
  instructions/     create_endowment, register_landlord, deregister_landlord, sweep, buyback,
                    count (begin/count/finish), close_contributions, pause, roles
  raydium.rs        Raydium CPMM account views, swap and deposit (layouts from idls/raydium_cp_swap.json)
  transfer.rs       hook-aware dividend transfers
  math.rs           buy sizing, post-close split, price floor, LP sizing, daily window
programs/endowment/tests/
  test_endowment.rs end-to-end tests: creation, isolation, opt-in, activation, sweeps, buybacks,
                    donations, liquidity, roles
  fixtures/         mainnet snapshots of the Raydium CPMM program and the PENIS/PUMP pool
```

## License

Apache-2.0. See [LICENSE](LICENSE).

Follow [@PenisEndowment](https://x.com/PenisEndowment).
