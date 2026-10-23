# Proposed tracked reward collection

Draft, not deployed. Requires the companion website/worker change and an owner-signed opt-in. It does not reset existing wallets or enable collection on either server.

## Problem and intended result

The legacy floor records PUMP held at enrollment. Spending those tokens can leave a holder below that floor indefinitely, even when subsequent PENIS rewards are independently verified. The workers already replay finalized transfers and consume reward eligibility before other funds; the fixed on-chain floor overrides that accounting.

Example (PUMP units): enroll with 100, spend 70, purchase 10, receive 20 in verified rewards. Balance is 60. The tracked path collects 20 into refundable custody and leaves 40. A later reward can be collected without refilling the original 100. Buying or returning 20 after spending a reward does not revive the spent reward.

The worker's accounting is:

- Start a new consent/history boundary with the current balance and zero eligible rewards; no historical backfill.
- Replay every finalized transaction in order. Only an attributed PENIS payout adds eligible PUMP.
- Every outflow consumes eligible rewards first, down to zero. Buys, ordinary incoming transfers and refunds add no eligibility.
- Reconcile the exact ending balance, and reread the finalized transaction cursor before signing. A net balance snapshot alone is insufficient.
- Missing, contradictory or ambiguous evidence retains the existing conservative stop/reset behavior. An understood contained swap resets that wallet's eligibility.

The amount left after deducting eligible rewards is the holder's protected remainder in this accounting model. It changes with spending and ordinary inflows. It is not the original enrollment balance.

## On-chain changes

`Landlord.reserved[0]` is now the owner-selected mode: 0 is legacy fixed-floor protection; 1 is tracked rewards. Account versions and lengths stay unchanged. Registration still starts in legacy mode. Only `enable_tracked_collection`, which requires the owner signature, can select tracked mode. Like existing `enable_collection`, it requires collection to be off, advances the consent epoch, clears allowance/attestations, and records the current balance. Existing pending receipts become refund-only across the consent change.

`authorize_collection(nonce, report, observation)` lets the configured reviewer create/update a `CollectionPermit` PDA at `["collection_permit", config, owner]`. No tokens move. It settles the current on-chain reward credit and refuses an amount above that credit, the delegated amount or the current balance. The reviewer funds the account rent once and later authorization fees.

The reviewer can replace a permit with a smaller amount, including zero to revoke it. Zero still requires the authorized reviewer, current consent/nonce/balance and matching observation context; it grants no positive collection authority. Spending and repurchasing the same balance must trigger a reduction or revocation after independent history replay, not leave the old higher grant until expiry.

Permit accounts are reused, not created per collection. This proposal does not close them after a holder leaves: the reviewer's rent stays in the per-wallet account, which holds no PUMP. Rent reclamation needs a separate lifecycle change that preserves nonce/consent replay protection. `Swept.baseline` remains the serialized Landlord field: a fixed floor in legacy mode, a checkpoint in tracked mode. Event consumers must check the consent mode; `CollectionHeld` retains its custody meaning.

A permit contains config/owner, collector/reviewer, consent epoch, next collection nonce, exact current balance, amount, deadline, completed count timestamp, refresher epoch, pause timestamp, UTC observation day, evidence hash and PDA bump. The reviewer signs the observed roles, count, refresher epoch, pause timestamp and day in the instruction; they must still match when it executes. A delayed authorization cannot adopt a newer boundary. Its maximum life is ten minutes, covering a five-minute worker interval and a slow pass, and it cannot cross midnight. Each actual sweep report still expires within sixty seconds.

For tracked holders, `sweep` requires this program-owned permit as the first remaining account. Every binding must match; the collector's requested amount cannot exceed the permit. The program then applies the existing balance, delegation, reward allowance, vault capacity, count, pause, goal and market checks. Accepted zero-amount and partial collections also consume the nonce, so unused permit capacity cannot be replayed. A successful token collection records the actual post-collection balance in `baseline` as a checkpoint, subsequently adjusted for refunds. It is no longer a fixed floor in this mode.

Legacy holders still use their fixed floor. An upgrade alone cannot opt them in; old workers cannot sweep a tracked holder without a permit. `enable_collection` remains an explicit way for an owner to return to legacy mode while collection is off.

## Trust boundary and remaining limits

The permit is an independent reviewer's attestation, not an on-chain proof of payout origin. The program cannot fetch and replay historical Solana transactions. The new mode trades the static floor for two independent checks before collection plus the existing review after collection. A common code/provider error, collusion, or compromise of both operators can cause incorrect collection within on-chain limits. The UI must disclose this change rather than promise the original fixed-floor protection.

A spend-and-rebuy can occur after the last finalized check but before execution, preserving the same balance. The collector's independent replay prevents using a stale permit when that history is already visible. If the race happens later, post-collection review must reduce clearance or refund the receipt. The permit never substitutes for that review. Refunds and ordinary transfers must remain excluded from subsequent eligibility.

The 24-hour minimum hold, fixed 72-hour refund deadline, owner reclaim, Stop, Revoke and existing role separation remain. No contract refund exists after release. Unlimited PUMP delegation and its wallet warning also remain.

Daily/count observation-boundary resets and read-retry policy are unchanged in this proposal. This fixes the permanent original-floor obstruction, not eligibility lost at those existing conservative boundaries. Carry-forward needs separate evidence/lifecycle review.

## Review and rollout

Review both repositories together. Contract tests execute legacy and tracked sweeps, owner-only mode changes, below-floor rewards, authorization ceilings, nonce replay, changed balance/consent/count/refresher/roles/pause, delayed authorizations across a boundary, UTC day changes, wrong permits, expiry, partial/no-op collection and refunds. A public Rust-generated fixture checks the client ABI. Worker tests cover full history replay, purchases/refunds, spend-rebuy races, missing history, stale feed, durable authorization outboxes and persistent reviewer quarantine even when eligibility is zero.

Regression tests also cover shrinking a same-balance permit, revoking it with zero even when reward credit is exhausted, normal wallet activity during authorization, and legacy re-enrollment while the tracked enrollment flag is on.

Before any rollout: review the new trust/consent model and program build, test on an isolated deployment, and update both workers. Existing on-chain accounts and journals must remain intact. New tracked consent starts a fresh boundary and requires new eligibility/count checks. Do not backfill Roy's earlier payouts or lower his live floor as part of this PR.

The companion UI flag defaults off; the reviewer must separately opt into authorization. Keep sweep and release disabled until the jointly agreed funded test. This branch does not perform any upgrade, wallet signature, server deployment or funded test.
