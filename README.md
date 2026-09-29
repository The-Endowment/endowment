# The $PENIS Endowment

Permanent capital. The only holder that can never pull out.

Website: [penis-endowment.vercel.app](https://penis-endowment.vercel.app) ([source](https://github.com/The-PENIS-Endowment/website))

This is a Solana program for **endowments**: vaults that hold a dividend-paying meme coin forever and turn every dividend they receive into more of it. The largest holders ("landlords") opt in by delegating their dividend rewards, and nothing else.

**One shared contract, many endowments.** Anyone can create an endowment for their own coin on this same deployed, audited program. $PENIS (paid in PUMP) is the flagship. Every endowment gets the same guarantees, enforced by the same code, and each one is fully isolated from the others.

## How it works

1. **Create.** Anyone calls `create_endowment` for a coin, its dividend asset, and the Raydium CPMM pool that trades one against the other. The program checks that the pool holds exactly those two mints, that both mints pass the mint policy (below), and that every parameter is within hard bounds. The creator becomes the admin, guardian and refresher unless others are named.
2. **Opt in.** A landlord signs one transaction: a standard token `Approve` on their dividend account (and only that account), naming the endowment's authority PDA as delegate, plus `register_landlord`, which records the dividend they already hold as their baseline. Landlords hold at least a minimum stake of the coin (0.1% of supply by default). There is no limit on the number of landlords.
   - **The wallet is the unit of commitment.** Everything in the wallet a landlord registers is committed: all its coin counts toward activation, and all new dividend arriving in it is swept. To commit only part of a holding, keep the rest in another wallet.
3. **Activation.** Landlord sweeps run once landlords together hold the activation share of the coin's supply (30% by default), and pause if that falls below the deactivation share (25% by default), or if no count has finished for three days. Once a day anyone can run the count: `begin_count`, then `count_landlords` in batches of any size, then `finish_count`. A count left open can be finished by anyone two hours after it began (or after a pause that overlapped it ended), with uncounted landlords counting zero.
   - **What a landlord counts for.** The smaller of what it holds now and what it held at its previous read, and only while it is still delegated, holds the round's minimum stake, and has been **attested** since its last count.
   - **Attestation.** Each endowment names a `refresher` (a timelocked parameter; the shared automation for the flagship). Between counts the refresher calls `refresh_landlords` over every landlord, at times nobody else chooses. The refresh only ever lowers each landlord's recorded balance to what it holds at that moment, and drops the record of any landlord it finds not delegated. A landlord only counts if the refresher has read it since its last count. So coin must be held from one count to the next, through the refresher's read, to count: coin moved between landlord wallets counts once, and a delegation held only around the count counts nothing. Anyone else may refresh too; only the refresher's reads attest. With no refresher, nobody counts and sweeps never switch on.
   - **Public.** Each landlord's record (counted amount, round, recorded balance, attestation) is on-chain, and each read is also published as an event.
4. **Sweep.** When a dividend drop lands, anyone can call `sweep`. It moves only the dividend above the baseline into the endowment's dividend vault and adds it to the landlord's on-chain contribution total. Only the landlord can move its own baseline (`resync_baseline`), which the website does whenever a landlord opts back in, so what a landlord already holds always stays theirs.
5. **Buy.** Anyone can call `buyback`, at most once per interval (10 minutes by default). The contract decides the size: whatever the vault holds, within the per-trade cap, a smoothly refilling daily allowance, and what the pool can absorb within the price-impact budget. It swaps that for the coin in the endowment's pool and pays the caller a small tip to cover network fees.
6. **The milestone.** Once an endowment has bought its milestone amount of the coin (200,000,000 $PENIS for the flagship), each buyback splits between buying the coin and adding permanent liquidity to the pool. Landlord contributions keep flowing.
7. **Opt out.** The landlord calls the token program's `Revoke`, and `deregister_landlord` to get its rent back. Neither can be blocked, even while the endowment is paused.

An endowment's own coin earns dividends too. They land directly in its dividend vault and are bought back like everything else.

## Guarantees in the code

- **No way out for the coin.** No instruction transfers tokens out of any endowment's coin vault, and every buyback checks that the vault's balance never goes down.
- **Liquidity is permanent.** LP tokens land in an account owned by the endowment's authority. No instruction can withdraw them.
- **Endowments are isolated.** Every endowment's config, authority, vaults and landlords are derived from its own address, and every account an instruction touches is derived from, or checked against, that endowment's config.
- **Nobody can squat an endowment.** An endowment's address includes its creator, so anyone else "creating" it only ever creates a separate endowment of their own. Pre-creating its vault token accounts can't block creation either.
- **Landlord exposure is limited.** A landlord's only exposure is dividend above their baseline in the one delegated account, and nobody but the landlord can lower that baseline.
- **The count is enforced on-chain.** Coin moved into a landlord wallet, borrowed for the count, or held by a landlord who revoked its delegation adds nothing until it has been held from one count to the next, and moving coin between landlord wallets counts it once, in whichever wallet held it when the refresher read them. A landlord that approves only for its own count counts nothing.
- **Fair prices.** Each buy is priced against the pool's time-weighted average price over at least 30 minutes of Raydium's own recorded price history, so nothing done in the same transaction, and no token sent straight into the pool, can move the floor. No single stretch of that history counts for more than half the average. A buy is refused if the spot price is more than 3% from that average in either direction, and every fill must clear `TWAP × (1 − fees) × (1 − max price impact)`, with the impact allowance between 0.1% and 3%.
- **Fail closed.** Buybacks stop, rather than overpay, if the pool fee exceeds 2%, if either coin's transfer fee (current or scheduled) exceeds 5%, if the pool disables swaps, if its data doesn't add up, if a vault is frozen, or if a transfer hook is switched on. Sweeps run the same checks, so while buybacks can't run, dividends stay in landlords' own accounts. After each Raydium call the program checks that nothing moved that shouldn't have, including the owner of every vault.
- **Mint policy.** An endowment's coin can have no mint authority, no freeze authority, and no permanent-delegate, pausable, non-transferable or confidential extension, and no transfer-hook program set: nobody can mint more, freeze it, or take it back out of the vault. The dividend asset can have none of those extensions either (a freeze authority is allowed: a frozen vault only pauses sweeps and buybacks). Transfer fees within the cap and metadata are fine. Both of the flagship's mints pass.
- **Bounded pacing.** The buy allowance refills at the daily cap per 24 hours and never holds more than one trade's worth, so no 24-hour window can spend more than the daily cap plus one trade.
- **One-way switches.** `retire` stops sweeps and new registrations for good without changing how buybacks spend, and `renounce_admin` freezes every parameter for good. Neither can move funds. Retiring waits out the same 72-hour timelock as a parameter change (the first call proposes it; the admin can withdraw it with `cancel_params`).
- **The guardian is a circuit breaker.** A pause blocks sweeps, counts, registrations and buybacks, lasts at most 7 days, can't be extended, and can't be repeated until 7 days after it ends. Only the admin can lift a pause early. Renouncing the admin also removes the guardian.
- **Every change is announced.** The admin proposes parameter changes (buy limits, spacing, tip, the post-milestone split, activation thresholds, minimum stake, refresher), always within bounds fixed in code. They take effect no sooner than 72 hours later, and anyone can see them coming. For its first day a matured change can only be applied by the admin (so a change the admin means to cancel can't be raced), then by anyone, and it expires seven days after maturing. Renouncing requires production activation thresholds and nothing pending, so sweeps can never be frozen on. The admin role can be handed over in two steps.

## The optional donation

When creating an endowment, a project can choose to donate **0%, 0.1%, 0.2% or 0.3%** of every buyback to the flagship $PENIS endowment, as thanks for the shared contract, website, automation and audit work. The rate is locked at creation and can never change. The donation is paid in the dividend asset and lands directly in the flagship endowment's dividend vault, where it is bought back into $PENIS like everything else. Nobody holds it.

Donations are only possible for coins that pay in the flagship's dividend asset (PUMP). The tip and the donation together are capped at 0.8% of each buy.

The flagship endowment's address is derived in the program from two constants: the $PENIS mint (`FLAGSHIP_COIN_MINT`) and the wallet that creates it (`FLAGSHIP_CREATOR`), exactly as every endowment's address is derived. **`FLAGSHIP_CREATOR` is a placeholder (all zeroes) until it is set before deploy.** While it is, no endowment can choose a donation, so nothing can ever be sent to an unowned address. A donation goes to the flagship's dividend vault while that exists and isn't frozen, and is skipped otherwise, so a donor's buybacks never depend on the flagship. The website checks its configured flagship against the same derivation.

## Use it, or run your own

- **Use the shared contract (recommended).** Create your endowment on this deployed program. You inherit its audit, its track record, the shared automation, and the explorer.
- **Deploy your own copy.** The code is open source under the Apache-2.0 license. You can deploy it yourself, but your copy won't share this contract's audit or track record.

## Cranking

Everything after opt-in is permissionless. The shared automation runs these, and anyone else can too:

| Instruction | Accounts, in order |
|---|---|
| `create_endowment(params)` | creator, config, authority, coin_mint, dividend_mint, dividend_vault, coin_vault, pool_state, coin_token_program, dividend_token_program, associated_token_program, system_program |
| `register_landlord` | owner, config, authority, landlord, dividend_mint, dividend_account, coin_mint, coin_account, dividend_token_program, coin_token_program, system_program |
| `resync_baseline` | owner, config, landlord, dividend_account |
| `sweep` | config, authority, landlord, dividend_mint, dividend_account, dividend_vault, coin_mint, coin_vault, pool_state, amm_config, pool_dividend_vault, pool_coin_vault, dividend_token_program |
| `buyback(min_out)` | config, authority, caller, caller_dividend_account, dividend_mint, coin_mint, dividend_vault, coin_vault, cpmm_program, cpmm_authority, amm_config, pool_state, pool_dividend_vault, pool_coin_vault, observation_state, lp_mint, lp_vault, flagship_dividend_vault, dividend_token_program, coin_token_program, lp_token_program, token_2022_program |
| `begin_count` | config, coin_mint |
| `count_landlords` | config, then for each landlord: landlord (writable), its coin_account, its dividend_account |
| `finish_count` | config |
| `refresh_landlords` | config (writable), caller (signer; the refresher's calls attest), then for each landlord: landlord (writable), its coin_account, its dividend_account |
| `apply_params` | caller (signer), config |
| `prune_landlord` | config, authority, landlord, owner, coin_mint, dividend_account, coin_account |

A `count_landlords` or `refresh_landlords` batch of 8 landlords fits a normal transaction; there is no limit on the number of batches. Landlord records closed since a batch was built are skipped. `buyback` passes `flagship_dividend_vault` read-only unless the endowment donates.

## Transfer hooks

A Token-2022 mint may carry a transfer-hook extension (PUMP's is currently unset). Raydium can't pass a hook's extra accounts, so buybacks, and the sweeps that feed them, pause while a hook is set on either mint, and resume if it's unset. Nothing is lost in the meantime: dividends simply stay in landlords' own accounts.

## Status

Pre-launch, in testing.

| Scope | State |
|---|---|
| Opt-in, sweeps, pause, permanent vaults | ✅ Built and tested |
| Buybacks: contract-sized, TWAP-priced, paced, tipped | ✅ Built and tested against mainnet pool state |
| Daily commitment count (unlimited landlords), milestone, post-milestone locked liquidity | ✅ Built and tested against mainnet pool state |
| Many endowments on one contract, optional flagship donation | ✅ Built and tested |
| Timelocked parameters, bounded pause, one-way retire and renounce | ✅ Built and tested |
| Test period with small caps, then the upgrade key is destroyed | Planned |

## Development

Requires Rust, the Solana CLI, and Anchor 1.1.2 (`avm install 1.1.2`).

```sh
./scripts/test.sh   # builds the program and IDL, and a test build, then runs every test
```

The integration tests run a build made with `--features test-flagship` (in `target/deploy-test`), which fixes a test flagship creator so donations can be exercised. `anchor build` alone produces the deployable program in `target/deploy`. Never deploy the test build.

Layout:

```
programs/endowment/src/
  lib.rs            instruction entrypoints
  state.rs          Config and Landlord accounts, the count round, parameters and their bounds, sweep math
  instructions/     create_endowment, register_landlord, resync_baseline, deregister_landlord,
                    prune_landlord, sweep, buyback, count, pause, roles (parameters, retire, admin)
  health.rs         whether an endowment can trade now: the checks sweeps and buybacks share
  mint_policy.rs    which coins and dividend assets an endowment can be created for
  raydium.rs        Raydium CPMM account views and the TWAP (layouts from idls/raydium_cp_swap.json)
  transfer.rs       fee caps and raw mint and token-account reads
  math.rs           buy sizing, price floor, allowance, post-milestone split, LP sizing
programs/endowment/tests/
  test_endowment.rs end-to-end tests: creation, isolation, opt-in, the count, refreshes, sweeps,
                    buybacks, donations, liquidity, parameters, roles, and audit regressions
  fixtures/         mainnet snapshots of the Raydium CPMM program and the PENIS/PUMP pool
```

## License

Apache-2.0. See [LICENSE](LICENSE).

Follow [@PenisEndowment](https://x.com/PenisEndowment).
