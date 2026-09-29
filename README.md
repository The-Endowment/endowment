# The $PENIS Endowment

Permanent capital. The only holder that can never pull out.

Website: [penis-endowment.vercel.app](https://penis-endowment.vercel.app) ([source](https://github.com/The-PENIS-Endowment/website))

This is a Solana program for **endowments**: vaults that hold a dividend-paying meme coin forever and turn every dividend they receive into more of it. The largest holders ("landlords") opt in by delegating their dividend rewards, and nothing else.

**One shared contract, many endowments.** Anyone can create an endowment for their own coin on this same deployed, audited program. $PENIS (paid in PUMP) is the flagship. Every endowment gets the same guarantees, enforced by the same code, and each one is fully isolated from the others.

## How it works

1. **Create.** Anyone calls `create_endowment` for a coin, its dividend asset, and the Raydium CPMM pool that trades one against the other. The program checks that the pool holds exactly those two mints, and that every parameter is within hard bounds. The creator becomes the admin unless another admin is named.
2. **Opt in.** A landlord signs one transaction: a standard token `Approve` on their dividend account (and only that account), naming the endowment's authority PDA as delegate, plus `register_landlord`, which records the dividend they already hold as their baseline. Landlords hold at least a minimum stake of the coin (0.1% of supply by default). Up to 28 landlords per endowment; when the roster is full, a larger holder can take the place of the smallest.
3. **Activation.** Landlord sweeps run once landlords together hold the activation share of the coin's supply (30% by default), and pause if that falls below the deactivation share (25% by default). Anyone can run `count_commitment` once a day. It reads **every** landlord in a single instruction, and counts each one for the smaller of what it holds now and what it held at the previous count, and only while it is still delegated. Coin has to be held for a full day to count, and can never be counted twice.
4. **Sweep.** When a dividend drop lands, anyone can call `sweep`. It moves only the dividend above the baseline into the endowment's dividend vault and adds it to the landlord's on-chain contribution total. Only the landlord can move its own baseline (`resync_baseline`), which the website does whenever a landlord opts back in, so what a landlord already holds always stays theirs.
5. **Buy.** Anyone can call `buyback`, at most once per interval (10 minutes by default). The contract decides the size: whatever the vault holds, within the per-trade cap, a smoothly refilling daily allowance, and what the pool can absorb within the price-impact budget. It swaps that for the coin in the endowment's pool and pays the caller a small tip to cover network fees.
6. **The milestone.** Once an endowment has bought its milestone amount of the coin (200,000,000 $PENIS for the flagship), each buyback splits between buying the coin and adding permanent liquidity to the pool. Landlord contributions keep flowing, and the endowment's own dividends keep working forever.
7. **Opt out.** The landlord calls the token program's `Revoke`, and `deregister_landlord` to get its rent back. Neither can be blocked, even while the endowment is paused.

An endowment's own coin earns dividends too. They land directly in its dividend vault and are bought back like everything else.

## Guarantees in the code

- **No way out for the coin.** No instruction transfers tokens out of any endowment's coin vault, and every buyback checks that the vault's balance never goes down.
- **Liquidity is permanent.** LP tokens land in an account owned by the endowment's authority. No instruction can withdraw them.
- **Endowments are isolated.** Every endowment's config, authority, vaults, roster and landlords are derived from its own address, and every account an instruction touches is derived from, or checked against, that endowment's config.
- **Nobody can squat an endowment.** An endowment's address includes its creator, so anyone else "creating" it only ever creates a separate endowment of their own. Pre-creating its vault token accounts can't block creation either.
- **Landlord exposure is limited.** A landlord's only exposure is dividend above their baseline in the one delegated account, and nobody but the landlord can lower that baseline.
- **The count can't be gamed.** One atomic count covers the whole roster, at most once a day. Coin moved between landlords, borrowed for the count, or held by a landlord who revoked its delegation adds nothing.
- **Fair prices.** Each buy is priced against the pool's 10-minute time-weighted average price, read from Raydium's own price history, so nothing done in the same transaction can move the floor. A buy is refused if the spot price is more than 3% above that average, and every fill must clear `TWAP × (1 − fees) × (1 − max price impact)`, with the impact allowance between 0.1% and 3%.
- **Fail closed.** Buybacks stop, rather than overpay, if the pool fee exceeds 2%, if either coin's transfer fee (current or scheduled) exceeds 5%, if the pool disables swaps, if its data doesn't add up, or if a transfer hook is switched on. After each Raydium call the program checks that nothing moved that shouldn't have.
- **Bounded pacing.** The buy allowance refills at the daily cap per 24 hours and never holds more than one trade's worth, so no 24-hour window can spend more than the daily cap plus one trade.
- **One-way switches.** `retire` stops sweeps and new registrations for good without changing how buybacks spend, and `renounce_admin` freezes every parameter for good. Neither can move funds.
- **The guardian is a circuit breaker.** A pause blocks sweeps, counts, registrations and buybacks, lasts at most 7 days, can't be extended, and can't be repeated until 7 days after it ends. Only the admin can lift a pause early. Renouncing the admin also removes the guardian.
- **Every change is announced.** The admin proposes parameter changes (buy limits, spacing, tip, the post-milestone split, activation thresholds, minimum stake), always within bounds fixed in code. They take effect no sooner than 72 hours later, and anyone can see them coming. Renouncing requires production activation thresholds, so sweeps can never be frozen on. The admin role can be handed over in two steps.

## The optional donation

When creating an endowment, a project can choose to donate **0%, 0.1%, 0.2% or 0.3%** of every buyback to the flagship $PENIS endowment, as thanks for the shared contract, website, automation and audit work. The rate is locked at creation and can never change. The donation is paid in the dividend asset and lands directly in the flagship endowment's dividend vault, where it is bought back into $PENIS like everything else. Nobody holds it.

Donations are only possible for coins that pay in the flagship's dividend asset (PUMP). The tip and the donation together are capped at 0.8% of each buy.

The flagship endowment's address is a constant in the program (`FLAGSHIP_CONFIG`). **It is a placeholder until the $PENIS endowment is created on mainnet**, and must be set to that endowment's config address, and the program redeployed, before the upgrade authority is burned.

## Use it, or run your own

- **Use the shared contract (recommended).** Create your endowment on this deployed program. You inherit its audit, its track record, the shared automation, and the explorer.
- **Deploy your own copy.** The code is open source under the Apache-2.0 license. You can deploy it yourself, but your copy won't share this contract's audit or track record.

## Cranking

Everything after opt-in is permissionless. The shared automation runs these, and anyone else can too:

| Instruction | Accounts, in order |
|---|---|
| `create_endowment(params)` | creator, config, authority, roster, coin_mint, dividend_mint, dividend_vault, coin_vault, pool_state, coin_token_program, dividend_token_program, associated_token_program, system_program |
| `register_landlord` | owner, config, authority, landlord, roster, dividend_mint, dividend_account, coin_mint, coin_account, dividend_token_program, coin_token_program, system_program, evict_landlord?, evict_owner? |
| `resync_baseline` | owner, config, landlord, dividend_account |
| `sweep` | config, authority, landlord, dividend_mint, dividend_account, dividend_vault, dividend_token_program |
| `buyback(min_out)` | config, authority, caller, caller_dividend_account, dividend_mint, coin_mint, dividend_vault, coin_vault, cpmm_program, cpmm_authority, amm_config, pool_state, pool_dividend_vault, pool_coin_vault, observation_state, lp_mint, lp_vault, flagship_dividend_vault, dividend_token_program, coin_token_program, lp_token_program, token_2022_program |
| `count_commitment` | config, roster, coin_mint, then each roster entry's coin_account and dividend_account, in roster order |

A count of a full roster touches 59 accounts plus the payer and program, within Solana's 64-account limit, and needs an address lookup table to fit the transaction size limit. It uses about 56,000 compute units.

## Transfer hooks

A Token-2022 mint may carry a transfer-hook extension (PUMP's is currently unset). Raydium can't pass a hook's extra accounts, so buybacks, and the sweeps that feed them, pause while a hook is set on the dividend mint, and resume if it's unset. Nothing is lost in the meantime: dividends simply stay in landlords' own accounts.

## Status

Pre-launch, in testing.

| Scope | State |
|---|---|
| Opt-in, sweeps, pause, permanent vaults | ✅ Built and tested |
| Buybacks: contract-sized, TWAP-priced, paced, tipped | ✅ Built and tested against mainnet pool state |
| Atomic commitment count, milestone, post-milestone locked liquidity | ✅ Built and tested against mainnet pool state |
| Many endowments on one contract, optional flagship donation | ✅ Built and tested |
| Timelocked parameters, bounded pause, one-way retire and renounce | ✅ Built and tested |
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
  state.rs          Config, Roster and Landlord accounts, parameters and their bounds, sweep math
  instructions/     create_endowment, register_landlord, resync_baseline, deregister_landlord,
                    prune_landlord, sweep, buyback, count, pause, roles (parameters, retire, admin)
  raydium.rs        Raydium CPMM account views and the TWAP (layouts from idls/raydium_cp_swap.json)
  transfer.rs       fee caps, raw token-account reads, hook-aware dividend transfers
  math.rs           buy sizing, price floor, allowance, post-milestone split, LP sizing
programs/endowment/tests/
  test_endowment.rs end-to-end tests: creation, isolation, opt-in, the count and roster, sweeps,
                    buybacks, donations, liquidity, parameters, roles, and audit regressions
  fixtures/         mainnet snapshots of the Raydium CPMM program and the PENIS/PUMP pool
```

## License

Apache-2.0. See [LICENSE](LICENSE).

Follow [@PenisEndowment](https://x.com/PenisEndowment).
