# The $PENIS Endowment

Permanent capital. The only holder that can never pull out.

Website: [thepenisendowment.com](https://thepenisendowment.com) ([source](https://github.com/The-Endowment/website))

This is a Solana program for **endowments**: vaults that hold a dividend-paying meme coin and reinvest the rewards they receive. The accepted V1 contribution policy covers all verified StonkFun rewards paid in PUMP to an enrolled holder while collection is active, including rewards from other coins. The reporting service must exclude purchased PUMP, existing balances, ordinary transfers and rewards already paid while inactive. See the [reward collection policy and trust model](docs/reward-routing-v1.md).

**Local implementation; public collection remains on hold.** Version 4 replaces balance-based sweeps with `collect_reward`, which requires the configured reporter's signature, fresh owner consent and an explicit amount. The old `sweep`, `register_landlord` and `resync_baseline` instructions reject. The reporter classifies receipts off-chain; the contract verifies authorization and collection conditions, not the origin of fungible PUMP. A faulty or compromised reporter can therefore collect purchased or pre-existing PUMP within the remaining delegation and treasury capacity. There is deliberately **no per-wallet daily cap**. The daily StonkFun comparison is an approximate sanity check, not proof of correct individual collections. Custom StonkFun routing is not required for this architecture.

The website release hold remains until review and an authorized rollout. Version 4 needs an initialized reporter policy and new owner consent; old approvals are not consent, and this branch has no migration instruction for live version 3 configurations. The companion reporting service and client must be reviewed together with this program before release.

**One shared contract, many endowments.** The program supports separate endowments for different coins. $PENIS (paid in PUMP) is the flagship. This review branch contains proposed changes and local test results; it is not a production deployment or independent audit certification.

## How it works

1. **Create.** Anyone calls `create_endowment` for a coin, its dividend asset, and the Raydium CPMM pool that trades one against the other. The program checks that the pool holds exactly those two mints, that both mints pass the mint policy (below), and that every parameter is within hard bounds. The creator becomes the admin, guardian and refresher unless others are named.
2. **Opt in.** After release, a landlord signs a standard token `Approve` on its dividend account (and only that account), naming the endowment's authority PDA as delegate, plus `enroll_rewards`. Enrollment requires an enabled reporter policy and assigns a new consent ID. The approval is broad and remains directly revocable through the token program. Landlords hold at least a minimum stake of the coin (0.1% of supply by default). There is no limit on the number of landlords.
   - **The wallet is the unit of commitment.** All its coin counts toward activation, and all qualifying StonkFun PUMP rewards are pledged, including PUMP rewards from other coins. The service starts eligible rewards at zero; the saved legacy baseline is not a collection authorization or balance floor. `renew_reward_consent` invalidates outstanding reports without moving tokens. To commit only part of a coin holding, keep the rest in another wallet.
3. **Activation.** Landlord sweeps run once landlords together hold the activation share of the coin's supply (30% by default), and pause if that falls below the deactivation share (25% by default), or if no count has finished for three days. Once a day anyone can run the count: `begin_count`, then `count_landlords` in batches of any size, then `finish_count`. A count can only begin once the refresher (below) has read someone since the last one began. A count left open can be finished by anyone four hours after it began (or after a pause that overlapped it ended), with landlords not yet counted counting zero.
   - **What a landlord counts for.** The smaller of what it holds now and what it held at its previous read, and only while it is still delegated, holds the round's minimum stake, and has been **attested** three times since its last count.
   - **Attestation.** Each endowment names a `refresher` (a timelocked parameter; the shared automation for the flagship). Between counts the refresher calls `refresh_landlords` over every landlord, in several passes a day at times nobody else chooses. Each read only ever lowers a landlord's recorded balance to what it holds at that moment, and drops the record (and the reads so far) of any landlord it finds not delegated. A landlord counts only after three such reads since its last count, each at least 30 minutes after the one before. So coin must be held from one count to the next, through every one of those reads, to count: coin moved between landlord wallets counts once unless it is moved ahead of each wallet's every read, and a delegation held only around the count counts nothing. A landlord still short of its reads when it comes to be counted is left pending, not counted as zero, so starting a count early can't zero anyone. Anyone else may refresh too; only the refresher's reads attest. With no refresher, nobody counts and sweeps never switch on.
   - **The refresher is a trusted role.** It can't move anything, but it chooses when it reads: a refresher that colluded with a landlord could time its reads to wherever that landlord's coin sits and count it more than once, or leave landlords out. So its reads are public, it is shown on every endowment's page, and it can resign at any time with `resign_refresher`, including after the admin has renounced. Resigning switches sweeps off at once, closes any open count, voids every read it made (reads are tagged with the endowment's refresher epoch), and removes it from any pending parameter change, so nothing it attested can count later and no proposal can bring it back. Nobody can hand the role to anyone else once the admin has renounced.
   - **Public.** Each landlord's record (counted amount, round, recorded balance, reads) is on-chain, and every refresher transaction publishes the landlords it read as an event.
4. **Collect reported rewards.** The trusted service matches finalized PUMP receipts with StonkFun distribution records, excludes purchases and inactive-period receipts, and reduces eligibility for spending before signing `collect_reward`. The contract binds the report to the endowment, registered source account, consent ID, collection and reporter epochs, next nonce, explicit amount, expected source balance and an expiry of at most 120 seconds. Only the fixed endowment dividend vault can receive the transfer. Oversized reports reject instead of being silently clipped. Collections cannot fill the vault beyond three days of what buybacks can actually spend (the smaller of the daily buy limit and what the per-buy limit allows at the minimum interval); this is treasury capacity, not a daily wallet limit. Pending eligibility is discarded when gaps or lifecycle changes prevent safe attribution. Balance equality does not protect against spending and replacing PUMP between observation and execution, so provenance remains trusted.
5. **Buy.** Anyone can call `buyback`, at most once per interval (10 minutes by default). The contract decides the size: whatever the vault holds, within the per-trade cap, a smoothly refilling daily allowance, and what the pool can absorb within the price-impact budget. It swaps that for the coin in the endowment's pool and pays the caller a small tip to cover network fees.
6. **The milestone.** The goal is the spendable coin held directly in the endowment's coin vault: 200,000,000 $PENIS for the flagship. Direct donations count; liquidity-pool holdings and cumulative purchase totals are reported separately. Every `collect_reward` checks this balance before collecting, and buybacks check it before and after trading. Reaching the goal permanently stops holder collections. A collection that records completion succeeds without transferring PUMP, so the completion flag persists. Once completion is recorded, new enrollments are rejected. Funds already in the endowment and future treasury yield continue through the configured buy/liquidity split.
7. **Opt out.** The landlord calls the token program's `Revoke`, and `deregister_landlord` to get its rent back. Neither can be blocked, even while the endowment is paused.

The intended ongoing funding source is the endowment's own rewards. The live distributor must confirm that the program-owned vault is eligible and that its rewards arrive in the intended dividend vault before public fundraising.

## Guarantees in the code

- **No way out for the coin.** No instruction transfers tokens out of any endowment's coin vault, and every buyback checks that the vault's balance never goes down.
- **Liquidity is permanent.** LP tokens land in an account owned by the endowment's authority. No instruction can withdraw them.
- **Endowments are isolated.** Every endowment's config, authority, vaults and landlords are derived from its own address, and every account an instruction touches is derived from, or checked against, that endowment's config.
- **Nobody can squat an endowment.** An endowment's address includes its creator, so anyone else "creating" it only ever creates a separate endowment of their own. Pre-creating its vault token accounts can't block creation either.
- **Collection authorization is explicit.** Only the configured reporter can authorize an amount from the registered dividend account, and only the derived endowment vault can receive it. Fresh consent, report epochs, nonce and expiry prevent stale or repeated authorizations. Source balance, remaining delegation and treasury capacity constrain transfers. There is no per-wallet daily cap and no on-chain proof that the reporter classified PUMP correctly; the baseline does not protect pre-existing or purchased PUMP against an incorrect report.
- **The count is enforced on-chain.** Coin moved into a landlord wallet, borrowed for the count, or held by a landlord who revoked its delegation adds nothing until it has been held from one count to the next, and through three of the refresher's reads. Moving coin between landlord wallets counts it once unless it lands ahead of every one of those reads in each wallet. A landlord that approves only for its own count counts nothing.
- **Fair prices.** Each buy is priced against the pool's time-weighted average price over 30 minutes of Raydium's own recorded price history (or, on a pool busy enough to fill Raydium's 100-record history sooner, all of it, and never less than 15 minutes), so nothing done in the same transaction, and no token sent straight into the pool, can move the floor. No single stretch of that history counts for more than a quarter of the average. Raydium records timestamps only to within a few seconds, so the reader also computes a bound on its own error for the history it read (typically well under 0.5%), and refuses to price from any history whose bound exceeds 3%. A buy is refused if the coin is pricier than that average by more than the endowment's price band (a timelocked parameter between 1% and 10%, 5% by default) plus the reader's bound; a coin that is cheaper than its average is simply a good price, and buys. Every fill must clear `TWAP × (1 − fees − half the max price impact − the band − the reader's bound − 0.1%)`, with the impact allowance between 0.1% and 3%, so however the price got where it is, the endowment never fills worse than that. Someone who pushes the price down just before a buy sells to the endowment cheaply and loses on the way back.
- **Fail closed.** Buybacks stop, rather than overpay, if the pool fee exceeds 2%, if either coin's transfer fee (in force or scheduled) exceeds 5%, if the pool disables swaps, if its data doesn't add up, if a vault is frozen, or if a transfer hook is switched on. Sweeps run the same checks, so while buybacks can't run, dividends stay in landlords' own accounts. After each Raydium call the program checks that nothing moved that shouldn't have, including the owner of every vault.
- **Mint policy.** An endowment's coin can have no mint authority, no freeze authority, and no permanent-delegate, pausable, non-transferable or confidential extension, and no transfer-hook program set: nobody can mint more, freeze it, or take it back out of the vault. The dividend asset can have none of those extensions either (a freeze authority is allowed: a frozen vault only pauses sweeps and buybacks). Transfer fees within the cap and metadata are fine, and a fee already above the cap is refused at creation. Both of the flagship's mints pass. Each keeps one third-party authority that could pause buybacks (never move funds): the $PENIS transfer-fee authority could raise its fee above the cap, and the PUMP transfer-hook authority could set a hook. Either would pause sweeps and buybacks until reversed, and the vault cap above bounds what could wait in the vault meanwhile.
- **Bounded pacing.** The buy allowance refills at the daily cap per 24 hours and never holds more than one trade's worth, so no 24-hour window can spend more than the daily cap plus one trade.
- **One-way switches.** `retire` stops collections and new enrollments for good without changing how buybacks spend. `renounce_admin` freezes parameters and reporter replacement, including any pending reporter proposal, for good. The reporter remains able to authorize collections or permanently disable itself; a lost reporter key cannot be replaced after renunciation. The refresher also remains live and can resign, but cannot be replaced. Retiring waits out the same 72-hour timelock as a parameter change (the first call proposes it; the admin can withdraw it with `cancel_params`), and like a parameter change the proposal expires seven days after maturing.
- **Reporter recovery is timelocked.** Before renunciation, the admin can propose a replacement reporter with a 72-hour delay. The admin or current reporter can disable collection immediately, invalidating reports and clearing any existing proposal. The guardian retains its bounded pause power and cannot disable the reporter indefinitely. Once disabled, repeated disable calls are harmless and cannot cancel the admin's subsequent recovery proposal. Only the admin's explicit `cancel_reporter` cancels that recovery. Applying a replacement invalidates old reports and re-enables collection.
- **The guardian is a circuit breaker.** A pause blocks sweeps, counts, registrations and buybacks, lasts at most 7 days, can't be extended, and can't be repeated until 7 days after it ends. Only the admin can lift a pause early. Renouncing the admin also removes the guardian.
- **Every configuration change is announced.** The admin proposes parameter changes (buy limits, price band, spacing, tip, the post-milestone split, activation thresholds, minimum stake, refresher), always within bounds fixed in code. They take effect no sooner than 72 hours later, and anyone can see them coming. For its first day a matured change can only be applied by the admin (so a change the admin means to cancel can't be raced), then by anyone, and it expires seven days after maturing. Reporter replacements use the same delay, apply grace and expiry. Renouncing requires production activation thresholds and a refresher unless already retired, plus no pending parameter or retirement proposal. A pending reporter proposal is frozen by renunciation. With an activation line set, the deactivation line is at least 0.01%, so a count that finds nobody always switches collections off. The admin role can be handed over in two steps.

These protections describe this program version. Retained program upgrade authority can change the code; renouncing an endowment's admin does not revoke the separate Solana program upgrade authority.

## The optional donation

When creating an endowment, a project can choose to donate **0%, 0.1%, 0.2% or 0.3%** of every buyback to the flagship $PENIS endowment, as thanks for the shared contract, website, automation and audit work. The rate is locked at creation and can never change. The donation is paid in the dividend asset and lands directly in the flagship endowment's dividend vault, where it is bought back into $PENIS like everything else. Nobody holds it.

Donations are only possible for coins that pay in the flagship's dividend asset (PUMP). The tip and the donation together are capped at 0.8% of each buy.

The flagship endowment's address is derived in the program from two constants: the $PENIS mint (`FLAGSHIP_COIN_MINT`) and the wallet that creates it (`FLAGSHIP_CREATOR`), exactly as every endowment's address is derived. **`FLAGSHIP_CREATOR` is a placeholder (all zeroes) until it is set before deploy.** While it is, no endowment can choose a donation, so nothing can ever be sent to an unowned address. A donation goes to the flagship's dividend vault while that exists, isn't frozen, the flagship is live (not paused or retired) and its coin can trade (no transfer hook, fee within the cap), and only up to the flagship's own vault cap, like a sweep; it is skipped (or trimmed) otherwise, so a donor's buybacks never depend on the flagship and nothing piles up where it can't be spent. The donating buy passes the flagship's coin mint and config as its last two accounts. The website checks its configured flagship against the same derivation.

## Use it, or run your own

- **Review the shared program.** This branch is pre-launch review code. No public collection, independent audit certification or production track record is claimed.
- **Review your own deployment.** The code is open source under the Apache-2.0 license. A separate deployment needs its own configuration, reporter operations, security review and release authorization.

## Cranking

Counts, buybacks and pruning are permissionless subject to their account and state checks. Collection requires the reporter signer; refresher attestations require the refresher signer. The table lists current entrypoints; the generated IDL is authoritative for signer and writable flags. Legacy `register_landlord`, `resync_baseline` and `sweep` reject:

| Instruction | Accounts, in order |
|---|---|
| `create_endowment(params)` | creator, config, authority, coin_mint, dividend_mint, dividend_vault, coin_vault, pool_state, coin_token_program, dividend_token_program, associated_token_program, system_program |
| `enroll_rewards` | owner, config, authority, landlord, dividend_mint, dividend_account, coin_mint, coin_account, dividend_token_program, coin_token_program, reporter_policy, system_program |
| `renew_reward_consent` | owner, config, landlord, dividend_account |
| `collect_reward(report)` | config, authority, landlord, dividend_mint, dividend_account, dividend_vault, coin_mint, coin_vault, pool_state, amm_config, pool_dividend_vault, pool_coin_vault, dividend_token_program, reporter_policy, reporter (signer) |
| `initialize_reporter(reporter)` | admin (signer), config, reporter_policy, system_program |
| `propose_reporter(reporter)`, `cancel_reporter`, `apply_reporter`, `disable_reporter` | caller (signer), config, reporter_policy |
| `buyback(min_out)` | config, authority, caller, caller_dividend_account, dividend_mint, coin_mint, dividend_vault, coin_vault, cpmm_program, cpmm_authority, amm_config, pool_state, pool_dividend_vault, pool_coin_vault, observation_state, lp_mint, lp_vault, flagship_dividend_vault, dividend_token_program, coin_token_program, lp_token_program, token_2022_program |
| `begin_count` | config, coin_mint |
| `count_landlords` | config, then for each landlord: landlord (writable), its coin_account, its dividend_account |
| `finish_count` | config |
| `refresh_landlords` | config (writable), caller (signer; the refresher's calls attest), then for each landlord: landlord (writable), its coin_account, its dividend_account |
| `apply_params` | caller (signer), config |
| `resign_refresher` | refresher (signer), config |
| `prune_landlord` | config, authority, landlord, owner, coin_mint, dividend_account, coin_account |

A `count_landlords` or `refresh_landlords` batch of 8 landlords fits a normal transaction; there is no limit on the number of batches. Landlord records closed since a batch was built are skipped. `buyback` passes `flagship_dividend_vault` read-only unless the endowment donates; a donating endowment's buyback passes it writable, plus the flagship's coin mint (`FLAGSHIP_COIN_MINT`) as the first remaining account.

## Transfer hooks

A Token-2022 mint may carry a transfer-hook extension (PUMP's is currently unset). Raydium can't pass a hook's extra accounts, so buybacks, and the sweeps that feed them, pause while a hook is set on either mint, and resume if it's unset. Nothing is lost in the meantime: dividends stay in landlords' own accounts, and what's already in the vault (at most three days of buys, whether swept or donated) is spent once trading resumes.

## Status

Pre-launch, in local review. Public enrollment and collection remain on hold.

| Scope | State |
|---|---|
| Version 4 consent, reporter-authorized collection, replay protection, reporter recovery | Implemented locally; regression and adversarial review in progress |
| Legacy balance-based registration, sweep and resync | Disabled on-chain |
| Reporter service and client consent flow | Companion local implementation; review with this program |
| Pause, owner exit, permanent vaults | Built and tested locally |
| Buybacks: contract-sized, TWAP-priced, paced, tipped | ✅ Built and tested against mainnet pool state |
| Daily commitment count (unlimited landlords), milestone, post-milestone locked liquidity | ✅ Built and tested against mainnet pool state |
| Many endowments on one contract, optional flagship donation | ✅ Built and tested |
| Timelocked parameters, bounded pause, one-way retire and renounce | ✅ Built and tested |
| Independent review, treasury reward eligibility and authorized pilot | Outstanding release prerequisites |
| Upgrade authority custody and eventual revocation | Community decision required before release; separate from endowment admin renunciation |

## Development

The local suite was run with Rust 1.89.0, Solana CLI 3.1.10 (platform-tools v1.52), and Anchor 1.1.2. Anchor 1.1.2 requests platform-tools v1.52 internally; newer build tools defaulting to SBPFv3 are incompatible with that compiler. This reproduces the local test environment; production deployment requires separate validation against the target cluster's supported bytecode versions.

```sh
./scripts/test.sh   # builds the program and IDL, and a test build, then runs every test
```

The integration tests run a build made with `--features test-flagship` (in `target/deploy-test`), which fixes a test flagship creator so donations can be exercised. `anchor build` alone produces the deployable program in `target/deploy`. Never deploy the test build.

Layout:

```
programs/endowment/src/
  lib.rs            instruction entrypoints
  state.rs          Config and Landlord accounts, the count round, parameters and their bounds
  reports.rs        reporter policy, report fields, consent/epoch/nonce/expiry validation
  instructions/     create_endowment, register_landlord, resync_baseline, deregister_landlord,
                    prune_landlord, collect_reward, reporter, buyback, count, pause, roles
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
