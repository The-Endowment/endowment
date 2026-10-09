# Version 4 launch safety

This is a coordinated **pre-deployment change**. It does not establish that the
program is audited, that rewards were correctly attributed, or that launch is
approved. No deployment or authority transfer is part of this change.

## Rules enforced by this program

- The allowance margin is exactly 10,000 basis points (1.0×), with a positive
  `max_rewards_per_day` no greater than 10 × `max_buy_per_day`. Creation,
  proposals and execution all validate this rule. The multiplication uses
  `u128` so large values cannot overflow the comparison. Sweeps always apply
  the allowance, with no uncapped branch.
- Participation parameters are either founders 0/0 or public 3,000/2,500 bps.
  Public creation locks public mode immediately. Applying the first public
  parameter proposal is irreversible: no later proposal can return to 0/0.
- Both modes require a nonzero completed-count timestamp no more than 72 hours
  old and not in the future. Founders mode bypasses only the participation
  threshold. A missing or stale count stops new collections and reward-index
  growth even if reward totals keep arriving. Completing a recovery count
  resets reward credit; the next post establishes a baseline without crediting
  the outage, even if no reward post observed the stale interval.
- That transition increments the attestation epoch, closes any open count,
  clears the last count and active flag, and invalidates carried founder
  allowance. A new count must reach 30%; an inherited 28% founder count cannot
  keep collection on. Subsequent hysteresis remains 30% on / below 25% off.
- A holder's allowance also requires the current attestation epoch and current
  count. An open count accepts both its already-processed holders and the
  immediately previous completed round; after completion, omitted holders
  receive no allowance. Recounting advances their index before restoring
  eligibility, so already-posted credit from the stale interval cannot return.
  This remains an estimate-based backstop, not proof of actual payout timing.
- The guardian's incident pause has no automatic expiry or cooldown. Only the
  admin resumes. A repeat incident can be paused immediately after resume.
- Every pending collection keeps its original `refund_at`, 72 hours after it
  was collected. An emergency pause permanently makes every pending receipt
  collected at or before `pause_started_at` refund-only, including receipts
  already approved. Anyone may refund these receipts immediately to the
  original holder. Review and release cannot restore them during or after
  resume. Later collections retain the normal 24-hour hold and 72-hour expiry.
- Admin renunciation requires completed retirement, no pending parameter or
  retirement proposal, and no active pause. Live collections retain the
  ability to rotate the collector, reviewer, refresher and guardian. The
  guardian cannot be removed before retirement. Operator changes retain their
  existing timelock and cannot name an arbitrary withdrawal destination.

## Intentional tradeoffs

An unresolved incident may stop buybacks indefinitely. That favors stopping
spending over liveness; admin custody must therefore have tested recovery.
Every incident cancels pending contributions instead of saving those
contributions for eventual buybacks. The cutoff uses seconds: a collection
made in the same second as pause/resume is conservatively refund-only. A later
pause never decreases the cutoff, even if chain time moves backwards.
Permissionless refund eligibility
still needs someone to submit and fund a transaction; alerts and a funded
refund service remain operational requirements.

The daily buy setting is a **refill rate**, not a strict rolling-day spend
ceiling. The token bucket can spend one initial transaction's allowance plus
24 hours of refill. The 10× rewards/buys ratio limits a configuration relationship,
not a dollar loss or accurate attribution. Values are PUMP base units.

Unlimited holder token delegation remains unchanged. A wrong reward post can
increase the allowance within its limits. A wrong collector report can still
collect unrelated PUMP above the baseline, within allowance and vault limits.
Independent payout/history review and holder reclaim reduce that risk while
funds are held; no receipt refund is available after release. Evidence hashes
commit to off-chain records but are not on-chain proof of their truth.

This does not enforce a founders wallet allowlist. The collector must be
configured to test only explicit participating wallets, and that operational
restriction is not protection from a compromised collector key. The public
site must remain closed until the reviewed public transition is executed and
verified. Timelocked proposals take at least 72 hours to apply.

Removing program upgrade authority is separate from renouncing admin. Before
removal, an upgrade can replace all these rules. Any retained upgrade authority
needs its own reviewed governance, delay and custody arrangements. There is no
automatic deadline for destroying it.

## Client compatibility

`CONFIG_VERSION` changes from 3 to 4; `LANDLORD_VERSION` remains 3. Account
sizes, discriminators and existing field offsets remain unchanged. Configuration
reserved byte 0 is now the irreversible public-launch flag, exposed through
`Config::public_launched()` and `lock_public_launch()`. The remaining ten bytes
remain reserved. Do not infer semantic compatibility from account length.

During an incident `Config.paused_until` is `i64::MAX` (9223372036854775807),
which is **not** a JavaScript-safe integer or a renderable calendar date. Clients
must decode it losslessly as bigint and display "paused until admin resume".
`unpause` replaces the value with the current chain timestamp. The receipt's
`refund_at` is the normal expiry; clients must remove the old pause-extension
calculation. `pause_started_at` remains a permanent cancellation cutoff. Route
any pending receipt with `collected_at <= pause_started_at` (when the cutoff is
nonzero) directly to `refund_collection`; do not attempt review or release.
The appended `CollectionInvalidated` error identifies a rejected incident
receipt. Account sizes, fields and instruction arguments do not change.

Regenerate the IDL and Rust-produced `holding-chain.json` fixture, and update
all website, worker and reconciliation version guards together. Existing v3
accounts are unsupported by this pre-launch change; any real deployment would
require a separate migration review.

## Validation

`launch_safety.rs` exercises rejected invalid allowance/ceiling configurations,
fresh public activation at 30%, rejection of return to founders mode, proposal
revalidation, loss-of-recovery prevention, and initialization before collection
policy creation. Existing pause/refund tests now check fixed expiry during an
indefinite pause and inability to revive an expired receipt after resume.
`count_outage.rs` checks actual reward posts during a count outage, recovery
with and without posts during the outage, first-count eligibility, exact
freshness boundaries, and reclaim while counts are stale. `incident_refunds.rs`
checks approved and unreviewed receipts across pause/resume, immediate
permissionless refunds, same-second cancellation, normal post-resume releases,
repeated incidents and a backwards-moving clock.

Older baseline and custody tests used an allowance-off configuration that is
now forbidden. Their explicit test-only `synthetic_allowance` fixture supplies
eligible credit to isolate those assertions. It is never part of the program.
The dedicated real reward-post/count allowance regressions and public launch
transition tests disable the fixture and exercise actual accrual.
