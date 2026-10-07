# The $PENIS Endowment

Permanent capital. The only holder that can never pull out.

Website: [peniscoin.meme](https://peniscoin.meme) ([source](https://github.com/The-Endowment/website))

This is the Solana program behind the **$PENIS Endowment**: a vault that holds $PENIS forever and turns the PUMP rewards it receives into more of it. The largest holders ("landlords") opt in by delegating their PUMP rewards, and nothing else.

**Open source, for any project.** The program, website and automation are open source under Apache-2.0. Any project with a dividend-paying coin is welcome to deploy its own copy and run its own endowment.

## How it works

1. **Create.** The creator (`FLAGSHIP_CREATOR`, set before deploy) calls `create_endowment` once, for $PENIS, its dividend asset (PUMP), and the Raydium CPMM pool that trades one against the other. The program checks that the pool holds exactly those two mints and that every parameter is within hard bounds. Nobody else can create an endowment on this program. The creator becomes the admin, guardian and refresher unless others are named.
2. **Opt in.** A landlord signs one transaction: a standard token `Approve` on their dividend account (and only that account), naming the endowment's authority PDA as delegate, plus `register_landlord` and `enable_collection`, which record the dividend they already hold as their baseline and switch collection on. Landlords hold at least a minimum stake of the coin (0.1% of supply by default). There is no limit on the number of landlords.
   - **The wallet is the unit of commitment.** Everything in the wallet a landlord registers is committed: all its coin counts toward activation, and eligible PENIS rewards arriving in it may be collected. To commit only part of a holding, keep the rest in another wallet.
3. **Activation.** Landlord sweeps run once landlords together hold the activation share of the coin's supply (fixed at 30% in public mode), and pause if that falls below the deactivation share (fixed at 25% in public mode), or if no count has finished for three days. Once a day anyone can run the count: `begin_count`, then `count_landlords` in batches of any size, then `finish_count`. A count can only begin once the refresher (below) has read someone since the last one began. A count left open can be finished by anyone four hours after it began (or after a pause that overlapped it ended), with landlords not yet counted counting zero.
   - **What a landlord counts for.** The smaller of what it holds now and what it held at its previous read, and only while it is still delegated with collection switched on, holds the round's minimum stake, and has been **attested** three times since its last count.
   - **Attestation.** The endowment names a `refresher` (a timelocked parameter; the endowment's automation). Between counts the refresher calls `refresh_landlords` over every landlord, in several passes a day at times nobody else chooses. Each read only ever lowers a landlord's recorded balance to what it holds at that moment, and drops the record (and the reads so far) of any landlord it finds not delegated. A landlord counts only after three such reads since its last count, each at least 30 minutes after the one before. So coin must be held from one count to the next, through every one of those reads, to count: coin moved between landlord wallets counts once unless it is moved ahead of each wallet's every read, and a delegation held only around the count counts nothing. A landlord still short of its reads when it comes to be counted is left pending, not counted as zero, so starting a count early can't zero anyone. Anyone else may refresh too; only the refresher's reads attest. With no refresher, nobody counts and sweeps never switch on.
   - **The refresher is a trusted role.** It can't move anything, but it chooses when it reads: a refresher that colluded with a landlord could time its reads to wherever that landlord's coin sits and count it more than once, or leave landlords out. So its reads are public, it is shown on the endowment's page, and it can resign at any time with `resign_refresher`, including after the admin has renounced. Resigning switches sweeps off at once, closes any open count, voids every read it made (reads are tagged with the endowment's refresher epoch), and removes it from any pending parameter change, so nothing it attested can count later and no proposal can bring it back. Nobody can hand the role to anyone else once the admin has renounced.
   - **Public.** Each landlord's record (counted amount, round, recorded balance, reads) is on-chain, and every refresher transaction publishes the landlords it read as an event.
4. **Collect and hold.** The endowment's collector calls `sweep` with a signed report: the exact amount, the account's exact balance, and an expiry a minute away. It moves only dividend above the baseline, and never more than the reward allowance, into a **holding account** that buybacks can't spend, and writes a receipt for it. Each receipt waits 24 hours. Then the endowment's reviewer, a separate key, approves it (`review_collection`) and anyone can release it to the dividend vault (`release_collection`); anything not approved goes back to the landlord. A receipt nobody releases within 72 hours can only go back (including time spent paused). Holding and vault together never exceed three days of what buybacks can actually spend; anything more waits in the landlord's own account. Only the landlord can move its own baseline (`resync_baseline`), so what a landlord already holds always stays theirs.
   - **A landlord can take any collection back.** Until a receipt is released, its landlord can reclaim it in full (`refund_collection`). Reclaiming a receipt from its current consent switches collection off until it opts in again; reclaiming an older receipt does not cancel a newer pledge. A refund by anyone else leaves the landlord enrolled, and every refund raises the landlord's baseline by what came back, so it is never collected again.
   - **A mandatory allowance, plus payout checks.** The allowance margin is fixed at 1.0× and cannot be disabled. Once a day the refresher posts cumulative holder rewards using `post_reward_total`; the bounded increase divided by supply adds allowance for each landlord's counted coin. Credit expires after at most 72 elapsed hours. The positive daily rewards ceiling can never exceed ten times the daily buy refill rate. These are limits on a trusted post, not proof of an individual payout: an incorrect post or spend/rebuy can still authorize collection of unrelated PUMP. The collector and independent reviewer must verify finalized payout and spending history; disputed funds stay out of buybacks. No protocol refund exists once a receipt has been released.

5. **Buy.** Anyone can call `buyback`, at most once per interval (10 minutes by default). The contract decides the size: whatever the vault holds, within the per-trade cap, a smoothly refilling daily allowance, and what the pool can absorb within the price-impact budget. It swaps that for the coin in the endowment's pool and pays the caller a small tip to cover network fees.
6. **The goal.** Once the endowment's coin vault holds its goal (200,000,000 $PENIS), whether bought or sent to it directly, it stops taking contributions for good: sweeps and new registrations end, and no count can switch them back on. Any dividend still in the vault is spent by buybacks as before (split by `buy_bps`, 100% buying for $PENIS). Sweeps and buybacks both record the goal.
7. **Opt out.** The landlord calls the token program's `Revoke`, and `disable_collection` or `deregister_landlord` (which returns its rent). None can be blocked, even while the endowment is paused, and anything still held can be reclaimed afterwards.

## Guarantees in the code

- **No way out for the coin.** No instruction transfers tokens out of the endowment's coin vault, and every buyback checks that the vault's balance never goes down.
- **Liquidity is permanent.** LP tokens land in an account owned by the endowment's authority. No instruction can withdraw them.
- **Every account is checked.** The config, authority, vaults and landlords are derived from the endowment's address, and every account an instruction touches is derived from, or checked against, its config.
- **Only the creator can create it.** `create_endowment` requires `FLAGSHIP_CREATOR`'s signature, and pre-creating the vault token accounts can't block creation.
- **Landlord exposure is limited.** A landlord's only exposure is dividend above their baseline in the one delegated account, and nobody but the landlord can lower that baseline. The mandatory allowance further bounds collection using the posted reward totals; it does not prove payout provenance. Every collection is signed by the collector, waits 24 hours in holding, and can be taken back by its landlord before release. Held dividend can only go to the endowment's vault or back to the landlord it came from: neither the collector nor the reviewer can send it anywhere else.
- **The count is enforced on-chain.** Coin moved into a landlord wallet, borrowed for the count, or held by a landlord who revoked its delegation adds nothing until it has been held from one count to the next, and through three of the refresher's reads. Moving coin between landlord wallets counts it once unless it lands ahead of every one of those reads in each wallet. A landlord that approves only for its own count counts nothing.
- **Fair prices.** Each buy is priced against the pool's time-weighted average price over 30 minutes of Raydium's own recorded price history (or, on a pool busy enough to fill Raydium's 100-record history sooner, all of it, and never less than 15 minutes), so nothing done in the same transaction, and no token sent straight into the pool, can move the floor. No single stretch of that history counts for more than a quarter of the average. Raydium records timestamps only to within a few seconds, so the reader also computes a bound on its own error for the history it read (typically well under 0.5%), and refuses to price from any history whose bound exceeds 3%. A buy is refused if the coin is pricier than that average by more than the endowment's price band (a timelocked parameter between 1% and 10%, 5% by default) plus the reader's bound; a coin that is cheaper than its average is simply a good price, and buys. Every fill must clear `TWAP × (1 − fees − half the max price impact − the band − the reader's bound − 0.1%)`, with the impact allowance between 0.1% and 3%, so however the price got where it is, the endowment never fills worse than that. Someone who pushes the price down just before a buy sells to the endowment cheaply and loses on the way back.
- **Fail closed.** Buybacks stop, rather than overpay, if the pool fee exceeds 2%, if either coin's transfer fee (in force or scheduled) exceeds 5%, if the pool disables swaps, if its data doesn't add up, if a vault is frozen, or if a transfer hook is switched on. Sweeps run the same checks, so while buybacks can't run, dividends stay in landlords' own accounts. After each Raydium call the program checks that nothing moved that shouldn't have, including the owner of every vault.
- **Token settings.** $PENIS has no mint or freeze authority, so nobody can mint more or freeze the vault. Each mint keeps one third-party setting that could pause buybacks (never move funds): the $PENIS transfer-fee authority could raise its fee above the cap, and the PUMP transfer-hook authority could set a hook. Either would pause sweeps and buybacks until reversed, and the vault cap above bounds what could wait in the vault meanwhile. A fee already above the cap is refused at creation.
- **Bounded pacing.** The buy allowance refills at the daily cap per 24 hours and never holds more than one trade's worth, so no 24-hour window can spend more than the daily cap plus one trade.
- **One-way switches.** `retire` stops sweeps and new registrations for good without changing how buybacks spend, and `renounce_admin` freezes every parameter for good, but only after retirement and while unpaused. Neither can move funds. Retiring waits out the same 72-hour timelock as a parameter change (the first call proposes it; the admin can withdraw it with `cancel_params`), and like a parameter change the proposal expires seven days after maturing.
- **Incident stops require explicit recovery.** The guardian can pause sweeps, counts, registrations, releases and buybacks indefinitely. Only the admin can resume; elapsed time never resumes operation. Refunds and holder exits remain available, and receipts become refund-only on their original 72-hour deadline. A new incident can be stopped immediately after resume. Removing the guardian is forbidden before retirement.
- **Bounded administration remains available while collecting.** Financial and operator parameters change through a 72-hour proposal. During the first day after maturity only the admin may apply; then anyone may apply, until expiry seven days after maturity. Validation runs at proposal and execution. The only participation settings are 0/0 founders mode and 30%/25% public mode. Public creation or the first public transition locks the latter permanently. That transition discards founder count/attestation results and old allowance credit: collection needs a fresh count reaching 30%. Live operator/guardian replacement cannot be destroyed by renouncing admin. A later decision to remove program upgrade authority is separate from retaining these bounded maintenance powers.

## The endowment's address

The endowment's address is derived in the program from two constants: the $PENIS mint (`FLAGSHIP_COIN_MINT`) and the wallet that creates it (`FLAGSHIP_CREATOR`). `FLAGSHIP_CREATOR` is pinned in `constants.rs`; deployment must independently verify the intended creator, program ID and derived configuration address. The website checks its configured endowment against the same derivation.

## Run your own

The code is open source under the Apache-2.0 license, and any project with a dividend-paying coin is welcome to run its own endowment: set your own mints and creator, deploy your copy, and run the automation from the [website repository](https://github.com/The-Endowment/website).

## Cranking

Counting, buybacks, releasing an approved receipt and returning an expired one are permissionless. Collecting needs the collector's signature, approving the reviewer's, and posting the reward total the refresher's. The admin can replace the collector and reviewer through the 72-hour timelock (`propose_collection_roles`, `apply_collection_roles`). The endowment's automation runs these; the generated IDL has every account list:

| Instruction | Accounts, in order |
|---|---|
| `create_endowment(params)` | creator, config, authority, coin_mint, dividend_mint, dividend_vault, coin_vault, pool_state, coin_token_program, dividend_token_program, associated_token_program, system_program |
| `register_landlord` | owner, config, authority, landlord, consent, dividend_mint, dividend_account, coin_mint, coin_account, dividend_token_program, coin_token_program, system_program |
| `resync_baseline` | owner, consent, config, landlord, dividend_account (switches collection off; `enable_collection` switches it back on) |
| `enable_collection` / `disable_collection` | owner, config, policy, consent, landlord, dividend_account / owner, consent |
| `sweep(nonce, report)` | collector (signer), policy, consent, receipt, pending_vault, system_program, config, authority, landlord, dividend_mint, dividend_account, dividend_vault, coin_mint, coin_vault, pool_state, amm_config, pool_dividend_vault, pool_coin_vault, dividend_token_program |
| `review_collection(approved_amount, evidence_hash)` | reviewer (signer), config, policy, receipt |
| `release_collection` / `refund_collection` | caller, config, policy, receipt, consent, rent_recipient, owner, landlord, dividend_mint, pending_vault, refund_account, authority, dividend_vault, coin_mint, coin_vault, dividend_token_program, coin_token_program, associated_token_program, system_program |
| `post_reward_total(total)` | refresher (signer), config, coin_mint |
| `buyback(min_out)` | config, authority, caller, caller_dividend_account, dividend_mint, coin_mint, dividend_vault, coin_vault, cpmm_program, cpmm_authority, amm_config, pool_state, pool_dividend_vault, pool_coin_vault, observation_state, lp_mint, lp_vault, dividend_token_program, coin_token_program, lp_token_program, token_2022_program |
| `begin_count` | config, coin_mint |
| `count_landlords` | config, then for each landlord: landlord (writable), its coin_account, its dividend_account, its consent |
| `finish_count` | config |
| `refresh_landlords` | config (writable), caller (signer; the refresher's calls attest), then for each landlord: landlord (writable), its coin_account, its dividend_account, its consent |
| `apply_params` | caller (signer), config |
| `resign_refresher` | refresher (signer), config |
| `prune_landlord` | config, authority, landlord, owner, coin_mint, dividend_account, coin_account |

A `count_landlords` or `refresh_landlords` batch of 6 landlords fits a normal transaction; there is no limit on the number of batches. Landlord records closed since a batch was built are skipped.

## Transfer hooks

A Token-2022 mint may carry a transfer-hook extension (PUMP's is currently unset). Raydium can't pass a hook's extra accounts, so buybacks, and the sweeps that feed them, pause while a hook is set on either mint, and resume if it's unset. Nothing is lost in the meantime: dividends stay in landlords' own accounts, and what's already in the vault (at most three days of buys) is spent once trading resumes.

## Status

Pre-launch, in testing.

| Scope | State |
|---|---|
| Opt-in, sweeps, pause, permanent vaults | ✅ Built and tested |
| Buybacks: contract-sized, TWAP-priced, paced, tipped | ✅ Built and tested against mainnet pool state |
| Daily commitment count (unlimited landlords), the 200M goal | ✅ Built and tested against mainnet pool state |
| Reward allowance (sweeps capped at what the coin earned) | ✅ Built and tested |
| 24-hour hold, review, and landlord reclaim | ✅ Built and tested |
| Timelocked parameters, persistent incident pause, retired-only renounce | ✅ Built and tested |
| Restricted founders pilot, independent review and recovery exercises before public launch | Required launch work |
| Removing program upgrade authority | Separate decision after acceptance criteria; no automatic burn date |

## Development

Requires Rust, the Solana CLI, and Anchor 1.1.2 (`avm install 1.1.2`).

```sh
./scripts/test.sh   # builds the program and IDL, and a test build, then runs every test
```

See [v4 launch safety and client compatibility](docs/launch-safety.md) before deploying or updating clients.

The integration tests run a build made with `--features test-flagship` (in `target/deploy-test`), which fixes a test creator so the endowment can be created. `anchor build` alone produces the deployable program in `target/deploy`. Never deploy the test build.

Layout:

```
programs/endowment/src/
  lib.rs            instruction entrypoints
  state.rs          Config and Landlord accounts, the count round, parameters and their bounds, sweep math
  instructions/     create_endowment, register_landlord, resync_baseline, deregister_landlord,
                    prune_landlord, sweep, rewards (the reward allowance), buyback, count, pause,
                    roles (parameters, retire, admin, refresher)
  health.rs         whether the endowment can trade now: the checks sweeps and buybacks share
  raydium.rs        Raydium CPMM account views and the TWAP (layouts from idls/raydium_cp_swap.json)
  transfer.rs       fee caps and raw mint and token-account reads
  math.rs           buy sizing, price floor, allowance, post-goal split, LP sizing
programs/endowment/tests/
  test_endowment.rs end-to-end tests: creation, isolation, opt-in, the count, refreshes, sweeps,
                    buybacks, liquidity, parameters, roles, and audit regressions
  regressions/      the reward allowance, the 200M goal, refresher changes
  fixtures/         mainnet snapshots of the Raydium CPMM program and the PENIS/PUMP pool
```

## License

Apache-2.0. See [LICENSE](LICENSE).

Follow [@PenisEndowment](https://x.com/PenisEndowment).
