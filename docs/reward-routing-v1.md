# V1 reported-reward collection design

The owner accepts a trusted reporting service for V1, with the daily StonkFun comparison as operational oversight. Custom distributor-funded routing is no longer a prerequisite for implementing collection. The pledge remains all verified StonkFun rewards paid in PUMP while enrolled and active, including rewards from other coins; purchases, existing balances, ordinary transfers and inactive-period receipts are excluded by the reporting policy.

This local implementation replaces balance-based collection with `collect_reward`. Legacy sweep, registration and baseline-resync instructions reject. Version 4 requires fresh owner consent and an initialized reporter policy; there is no migration instruction for live version 3 configurations. The website release hold remains until independent review and an authorized rollout. The original review snapshots remain preserved. This replacement is included in the final PR 2 package, with the service/client in PR 3.

## Accepted trust model

The reporting service determines which receipts qualify and how much eligible PUMP remains. It uses StonkFun's public distribution records and finalized Solana transactions, not a wallet's balance increase alone. The contract authenticates the reporter and enforces bounded withdrawal conditions; it cannot independently prove that an off-chain classification is correct.

A reporter error, compromised signing key or a race between its observation and execution can cause an incorrect collection. A matching daily aggregate does not prove that every wallet was charged correctly. The owner explicitly chose no per-wallet daily cap so high-volume rewards can all contribute. Remaining token delegation and treasury capacity constrain transfers, but there is no daily wallet loss limit. Retained program upgrade authority is a separate power to change contract protections.

The authorized destination is always the derived endowment PUMP vault. The reporter cannot select another recipient, withdraw PENIS or spend treasury assets through the collection instruction. It must not receive a holder's private key. Keep its collection authorization separate from the refresher and ordinary fee-paying keeper roles.

## Contribution policy

| Funds or state | Treatment |
| --- | --- |
| Verified StonkFun PUMP rewards, from PENIS or other reward coins | Eligible while enrolled and collection is active |
| PUMP held before enrollment, purchased later or ordinarily transferred in | Never added to the service's eligible balance |
| STONK, PENIS or other non-PUMP rewards | Excluded |
| Rewards already paid before enrollment/activation or while inactive | Permanently excluded; no later catch-up |
| Outgoing PUMP, including spending, transfers and burns | Reduce the service's eligible balance first |
| Missing history, ambiguous transactions or uncertain eligibility | Discard uncertain eligibility and report the gap; never infer a debt from the current balance |
| Exit | Stops future collection after the owner's on-chain action; completed contributions are not refunded |
| Re-enrollment | New consent generation; previous reports cannot be reused |
| Direct treasury PENIS reaches 200 million | Holder collection stops permanently; LP holdings do not count |

Activation remains 30% of PENIS committed. An active campaign suspends below 25% and resumes at 30%; exactly 25% does not suspend it. Counts remain sampled observations. Pauses, stale counts, retirement, completion, token/market checks and treasury capacity must also gate collection.

Eligibility uses payout execution time. Rewards accrued earlier but first paid after activation can qualify; already-delivered inactive-period rewards cannot. StonkFun's public disclaimer allows some rounds to pay a launch's own coin, so verify the actual reward mint instead of assuming every payout is PUMP.

## Forward-only reporting

No pre-launch payout history is required. Each enrollment starts an eligible balance of zero and a fresh observation boundary. Persist subsequent transaction identifiers, consent/lifecycle boundaries, classifications, report nonces and confirmed collections. A compact current ledger plus an append-only decision log supports restart and review without scanning the wallet's lifetime history.

For each monitored PUMP account:

1. Recognize a payout using the approved distributor/source policy, the actual PUMP mint, successful finalized transaction transfers and the matching StonkFun distribution record. Generating-coin labels are reporting metadata rather than a PENIS-only eligibility filter. The public recent-record feed is bounded, so missing a match leaves a reward uncollected.
2. Add only net credited PUMP from an eligible receipt to the pending eligible amount. Purchases and other credits never increase that amount.
3. Reduce eligibility for outgoing PUMP first, down to zero. A confirmed endowment collection is counted once as an outflow. This deliberately favors leaving funds with holders when the origin of spent tokens is ambiguous.
4. If a transaction mixes unresolvable inflows/outflows, an observation gap occurs, a token account closes or consent/activation becomes uncertain, clear pending eligibility and start from a fresh verified boundary. Do not restore discarded eligibility on a later purchase or deposit.
5. Clear outstanding authorizations on exit, pause, stale-count suspension, completion, reporter rotation or re-enrollment. The simplest recovery is to leave previously pending rewards with the holder when an interruption prevents safe collection.
6. Before requesting collection, refresh transaction coverage and account state. Produce a short-lived, wallet-specific report for at most the remaining eligible amount and available contract allowance. Log the underlying payout IDs and all reductions so a reviewer can reproduce the amount.

A service may learn of a candidate payout from a webhook or API update, but neither notification alone is sufficient evidence. A token account balance, daily estimated share or treasury payout timing cannot substitute for receipt classification.

## Contract-enforced bounds

Replace the old permissionless balance-surplus sweep with an instruction authorized by the configured reporter signer. The first implementation can require the reporter to sign the actual Solana transaction, avoiding a separate signature-verification protocol. A fee payer is not automatically a reporter.

Each authorization must bind the program/endowment instance, owner, registered PUMP token account, consent generation, current collection-state generation, reporter generation, unique monotonic nonce, explicit amount and short expiry. Use a persistent consent/nonce record so closing and recreating an enrollment cannot resurrect old reports. Define the migration and rent treatment explicitly before changing account layouts.

The instruction must check:

- The exact configured reporter signer, with no permissionless legacy fallback.
- Owner consent, current SPL delegation, correct source owner/mint/token program and the fixed derived treasury destination.
- A positive explicit amount, fresh authorization and unused nonce. The reporter does not get a “sweep the balance” option.
- Remaining token delegation and existing treasury capacity. There is no per-wallet daily cap. Reports are rejected, never silently clipped, if the explicit amount cannot be transferred.
- Current activation/count freshness, guardian pause, retirement, tradeability and actual direct-vault completion. Reports from a previous consent, reporter or collection generation must remain unusable after a restart.
- Account state has not changed from any conservative precondition included in the report. A balance equality check is useful but is not proof against spending followed by a purchase that restores the same balance; this remains a limitation of asynchronous reported collection.
- Atomic consumption of the nonce, actual net vault receipt and accounting changes only on a successful transfer. A failed transfer must not count as a contribution. Never silently increase the reported amount to fill the treasury or make a daily estimate match.

Reporter rotation should use the existing governance/timelock pattern and invalidate outstanding reports. Emergency disabling can only stop collections; resumption must not revive old reports. Owners must be able to revoke their delegation or exit without the reporter, website, guardian or admin. Choosing and publishing the reporter key/custody, expiry and upgrade authority policy are required before deployment, not reasons to depend on a custom StonkFun interface.

## Implementation sequence

1. Add the bounded reporter-authorized instruction, persistent consent/replay state and owner exit. Remove or disable the old sweep on-chain. Keep completion, tradeability and treasury capacity checks shared with the existing protections.
2. Implement the forward-only receipt ledger and reporter signing service. Separate observation/classification from signing; persist decisions before submission and reconcile finalized execution afterward. A daily report discrepancy is an investigation signal, never a debit authorization.
3. Replace enrollment with clear trusted-service consent without a daily wallet cap. Update count/refresher/pruning assumptions and all affected layouts/IDL/client decoders together. Previously granted broad approvals must not be treated as consent to the changed policy.
4. Add end-to-end adversarial tests, independent review and an explicitly authorized small pilot. Only then remove the website's collection hold. Include this behavior change in the contract PR 2 and the paired client/service PR 3; preserve PR 1 as the independent count fix.

No live keys, deployment or pilot are authorized by the trust-model decision. Changes remain local for review.

## Acceptance scenarios

| Scenario | Required outcome |
| --- | --- |
| Start with 1,000 PUMP; receive 200 while waiting; buy 300; receive 100 while active | Service authorizes at most 100; inactive receipts and purchases do not increase eligibility |
| Receive 100 eligible PUMP; spend it; buy 100 PUMP | Eligibility remains zero after the purchase |
| Receive rewards from two StonkFun coins, both in PUMP | Both can qualify; a STONK or PENIS payment cannot |
| Coverage gap or unresolvable mixed transaction | Discard uncertain eligibility and report the gap; never recover it from a later balance |
| Wrong reporter, destination, mint, owner, instance or source account | Reject before moving funds |
| Repeat a report, change its amount or reuse it after exit/rejoin | Reject replay or consent mismatch |
| Old report after pause/resume or reporter rotation | Reject the previous generation even if its wall-clock expiry has not passed |
| Amount exceeds the owner's allowance or treasury capacity | Reject oversized reports; the service may prepare a smaller explicit amount and retain verified remainder |
| Goal reached by donation before collection | No holder PUMP moves; completion remains permanent |
| Token transfer fails | No contribution or consumed report is recorded |
| Reporter supplies a false but otherwise valid amount | Contract bounds still apply; document that provenance correctness is trusted, not independently proved |
| Daily totals happen to match despite a wallet-level mistake | Do not label the aggregate report as proof of correct individual collection |

## Disclosure before enrollment

Explain that a project-operated service identifies eligible StonkFun PUMP rewards and authorizes collection. The intended policy excludes existing/purchased PUMP, ordinary transfers and inactive-period rewards, but the contract trusts that service's classification. An error or compromise can cause incorrect collection subject to token allowance and treasury capacity. Explain the absence of a daily wallet cap, the possibility of missed contributions, the fixed treasury destination, exit, upgrade authority and the permanent 200M direct-vault goal in plain language.

Do not publish unconditional claims that purchased PUMP “cannot” be taken under this model. Direct distributor-funded routing can remain a later way to reduce trust; it is not required to implement the now-accepted V1 architecture. Program-owned treasury reward eligibility remains a separate fact to verify before launch.

## Implemented account and consent details

Config and Landlord keep their allocation sizes and existing field offsets, consuming reserved bytes for version 4 fields. A config-wide monotonic consent ID prevents replay after deregistration or pruning closes an enrollment. `renew_reward_consent` replaces the consent ID and observation time without silently opting an old version 3 record into the policy. Report nonces remain monotonic within an enrollment. `baseline` is historical data and never controls the collected amount. Reports expire within 120 seconds and bind the observed source balance; spending and rebuying to the same balance remains a trusted-reporter race.

Every finalized commitment count, parameter application, guardian pause and refresher invalidation advances the collection epoch. Previously pending eligibility is discarded at these boundaries. A direct-vault completion latches before any collection and can return successfully without consuming the report or transferring tokens. Reporter replacement requires 72 hours, has a one-day admin grace and seven-day expiry, and advances the reporter epoch. Admin or the current reporter can disable immediately; the guardian retains its bounded pause only. The first disable clears pending replacement; repeated disables are harmless and cannot veto a new recovery proposal. Restarting requires a new timelock. Renouncing the admin freezes reporter replacement even if a proposal remains recorded, while the reporter can still disable itself permanently.
