# Refundable collection — draft implementation

This branch extends Brett's `allowance-v1` / endowment PR #3. It is a proposal,
not a deployed protocol or a completed independent security audit. It replaces
the legacy direct-to-treasury sweep and requires fresh signed consent. The
companion website branch is `codex/refundable-collection-client`.

## What is enforced

There are two PUMP token accounts. The existing treasury authority owns the
spendable account. A new, immutable collection-policy PDA owns the holding
account; buyback has no way to sign for it. A permanent PENIS vault remains the
only balance used for the 200M goal. Treasury-owned rewards and intentional
direct treasury donations are distinct from holder collections.

Every successful nonzero collection creates a `PendingCollection` account:
config, original holder, original rent payer, monotonic nonce, consent epoch,
exact amount, collection timestamp, release timestamp, expiry timestamp,
collection evidence hash, approval amount and review evidence hash. The config
fixes the reward mint. Fee-bearing dividend mints are rejected, so amount debited
must equal amount held and refunded. Each receipt has its own timer:

- Earliest review/release: collection time + 86,400 seconds.
- At collection time + 259,200 seconds: refund only, even if already approved.
- Before release, the holder can reclaim the full receipt, including an approved
  one. Release/reclaim are competing transactions; whichever executes first wins.
- The reviewer can reject/refund at any time. Anyone can refund after expiry,
  cancellation, retirement, or actual goal completion.
- Review may clear less than the receipt. Release transfers the approved portion
  to the fixed spendable treasury and the remainder to the original holder.
- A receipt closes on settlement; its rent returns only to its original payer.
  Its nonce is never reusable, even after deregistration and re-enrollment.
- No new collection at the goal; existing pending funds become refundable rather
  than being committed after the goal is met. Pauses block release, not recovery.

A shared holding balance does not create shared permission to spend it. For
example, 1,000 old approved PUMP plus 500 newly collected PUMP permits a release
of only 1,000. The remaining receipts stay backed. `policy.pending` is the total
reserved liability; each settlement subtracts exactly its own receipt amount.
Unsolicited transfers into this holding account create no receipt or withdrawal
right. Use the designated permanent treasury for intentional direct donations.

## Consent, activation and refund protection

Registration creates a durable `CollectionConsent` PDA but leaves it disabled.
An explicit owner-signed `enable_collection` protects the current PUMP balance,
clears old reward allowance and advances the consent epoch. Only that owner can
enable it. A pasted wallet address cannot consent on someone's behalf.

Any nonzero refund disables collection and advances the epoch. Enabling again
protects the refunded balance; older pending receipts have an obsolete epoch
and remain refund-only. Resync and deregistration also invalidate consent.
The durable record is never closed, so leaving/rejoining cannot restore an old
nonce or erase a refund revocation. Its small rent deposit is intentionally not
returned on deregistration. Receipt rent is returned on settlement.

Counting and refresh now require a fourth account per holder: that consent PDA.
Disabled consent counts zero even if SPL delegation still exists. Existing
30%/25% hysteresis and count freshness rules are retained. Counts remain sampled
observations, not a continuous oracle of every wallet's current holdings.
The companion keeper uses six holders per batch to stay within packet size.

Revoking SPL delegation independently still stops collection immediately.
`disable_collection` needs only the owner and durable consent, so market/keeper
failures cannot block it. Reclaim does not depend on a live landlord record,
active commitment count, outstanding delegation, or the original token account
remaining open. A missing canonical refund ATA is recreated by the caller.

## Trust and evidence

`initialize_collection` can run only once, signed by the config admin. It names
distinct nonzero collector and reviewer keys. Those keys cannot be rotated by
this version. The collector cannot approve; the reviewer cannot collect. Neither
can choose a withdrawal destination or shorten the hold. Different keys alone
do not establish operational independence: use separate operators/secrets and
separate independently collected evidence stores.

The collector attests an exact maximum amount, current wallet balance, consent
epoch, nonce, expiry (at most 60 seconds), and evidence hash. On-chain baseline,
delegation, upstream optional allowance and combined pending+treasury exposure
limits further restrict collection. An accepted zero-amount report consumes its
nonce but leaves no funded receipt, preventing ambiguous keeper retries.

**These are attestations, not on-chain proofs of reward origin.** The contract
cannot authenticate the Stonk API or distinguish fungible PUMP by purchase
history. A same-balance spend/rebuy can evade the collection balance check.
The companion collector/reviewer independently replay finalized source history,
credit only PENIS-labelled distributor payouts confirmed by API batch records,
and consume eligible rewards on outgoing transfers. Uncertainty gives no new
credit; insufficient evidence causes refund rather than release. Historical
records must be archived while public API records are still available.

A collector compromise can temporarily collect ineligible PUMP within on-chain
bounds; the holder can reclaim it and an honest reviewer should refund it. A
reviewer compromise cannot increase a receipt, but could wrongly approve funds
the collector already captured. Compromising both services, their shared API/RPC
trust, or the program upgrade authority defeats those protections. The upstream
allowance is an estimate, and never proves purchased PUMP is safe.

On-chain receipts record when funds entered custody. Off-chain evidence records
each payout signature, source-wallet arrival chain time, amount and subsequent
spending. A sweep may cover several payouts. The evidence hash links these
records; it is not their public storage or a guarantee they will remain available.
Operators must retain and make the audit evidence available for review.

## External mint limitation

The checked-in PUMP fixture is Token-2022 with an inactive but controllable
transfer hook. Activating a hook could prevent collections, releases and refunds
until transfers are possible again. Expiry cannot override another token
program's controls. This contract adds no hook bypass or fee-loss exception.
An exact refund will fail atomically if the destination receives less than the
recorded amount. Reverify the live mint before any launch.

## Defaults to agree before launch

The holder's right to reclaim was explicitly approved. This draft proposes:
PENIS-only PUMP rewards; independent immutable collector/reviewer roles; a
24-hour hold; 72-hour refund-only deadline; refund all remaining pending funds at
the actual goal; no fee-bearing dividend mint. These choices are reviewable
proposals, not a claim that earlier discussions settled them.

The allowance parameters from PR #3 remain as additional optional bounds. They
still have its estimation/carry-over limitations. Decide their production values
explicitly; this PR does not turn an approximate allowance into evidence.

## Integration and validation

Integration path under review: endowment `main -> PR #3 -> this PR`; website
`main -> website PR #1 -> refundable-collection-client`. The older endowment PR
#2 is an alternative architecture, not a prerequisite to merge blindly here.
No existing deployment is upgraded and no keys are loaded by the tests. This is
a fresh-deployment ABI: existing registrations require a reviewed migration or
re-enrollment. Do not update only the website or only the program.

Build with the compatible Solana 3.1.10/Anchor 1.1.2 toolchain and run
`./scripts/test.sh`. It checks both SBF builds for stack overflow diagnostics and
runs unit and LiteSVM transaction tests. New tests exercise independent timers,
early/oversized/unauthorized approval, stale/replayed reports, legacy bypass,
partial refund, redirected refund, token-account recreation, durable consent,
re-enrollment, pause/retirement, donations reaching the goal, and disabled consent
counting zero. The original buyback/price/refresher tests remain in the suite.

To regenerate the companion client's cross-language fixture, run:

```sh
HOLD_FIXTURE_PATH=/absolute/path/to/website/tests/fixtures/holding-chain.json \
  cargo test -p endowment export_cross_language_hold_fixture
```

This is test-only public account data. Never deploy `test-flagship` binaries.
Still required before launch: agree policy/role custody, verify that this
program-owned treasury actually receives Stonk rewards, exercise independent
services on a test deployment with outages, and obtain an independent security
review. Submission and enrollment remain disabled in the companion application.
