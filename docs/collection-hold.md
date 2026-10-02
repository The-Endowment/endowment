# Collection: hold, review and reclaim

How dividend (PUMP) gets from a landlord's account to the endowment's buyback vault. Based on the design in PR #4.

## The path of one collection

1. **Collect.** The collector signs `sweep(nonce, report)`. The report names the exact amount, the account's exact balance, the landlord's consent epoch, and an expiry at most 60 seconds away. The contract then takes the smallest of:
   - the report's amount;
   - what sits above the landlord's baseline;
   - the landlord's reward allowance (what its counted coin earned, 1.0×, carried over at most 72 elapsed hours from its reward post);
   - the room left under the vault cap (holding and vault together).
2. **Hold.** The dividend goes to a holding account owned by the collection-policy PDA. Buybacks can't sign for it. A receipt records the landlord, amount, time, and the collector's evidence hash. Each receipt has its own 24-hour timer.
3. **Review.** After 24 hours the reviewer approves an amount up to the receipt (`review_collection`).
4. **Release.** Anyone can then call `release_collection`. The approved amount goes to the dividend vault; the rest goes back to the landlord.
5. **Or refund.** `refund_collection` sends the whole receipt back to the landlord's own token account. It can be called by:
   - the landlord, at any time before release;
   - the reviewer, at any time;
   - anyone, once the receipt is 72 hours old, the landlord's consent has changed, the goal is reached, or the endowment is retired.

Held dividend has two possible destinations: the endowment's vault, or the landlord it came from. No caller chooses another.

## What a refund does

- **The landlord reclaims a current receipt:** its collection switches off until it calls `enable_collection` again. Its other pending receipts then can only be refunded.
- **Anyone else refunds, or a release approves only part:** the landlord stays enrolled.
- **Every refund** raises the landlord's baseline by what came back, so it is never collected again, and takes it out of the global contribution total. It adjusts the current landlord total only if the receipt belongs to that registration; refunds from before a leave/rejoin cannot erase new contributions. Consent renewal within one registration still adjusts that registration.

## Timing

| | |
|---|---|
| Report valid for | at most 60 seconds |
| Hold before review and release | 24 hours |
| Refund-only after | 72 hours |
| A pause | blocks release, never a refund; only a pause beginning before the original expiry can extend the review window; expired receipts never reopen |

## Roles

| Role | Can | Can't |
|---|---|---|
| Collector | Sign a collection, within every limit above | Approve, release early, or pick a destination |
| Reviewer | Approve up to a receipt's amount after 24 hours; refund | Collect, raise an amount, or pick a destination |
| Admin | Replace collector and reviewer, 72 hours after proposing it | Move held dividend; act after renouncing |
| Landlord | Reclaim any pending receipt; stop collection; revoke the delegation | |

The evidence hashes commit to the payout and spending records each service keeps. The contract's own limits (baseline, allowance, vault cap) hold whatever the services report.

## Consent

`register_landlord` creates a consent record, switched off. `enable_collection`, signed by the landlord, sets the baseline to the current balance, restarts the allowance, and switches collection on. `disable_collection`, `resync_baseline` and `deregister_landlord` switch it off. A landlord counts toward activation only while it is on. The record is kept after leaving, so nonces are never reused.

## Totals

- `CollectionPolicy.pending`, `.released`, `.refunded`: where every collected unit is or went.
- `Config.total_swept` and `Landlord.total_contributed`: collected and not refunded.
- Events: `CollectionHeld`, `CollectionReviewed`, `CollectionSettled`, `CollectionRolesProposed`, `CollectionRolesChanged`.

## Tests

`tests/regressions/collection_hold.rs` and `collection_attacks.rs`. To regenerate the cross-language fixture for the worker:

```sh
HOLD_FIXTURE_PATH=/absolute/path/to/holding-chain.json cargo test -p endowment export_cross_language_hold_fixture
```

## Corrections from review (#6)

The `hold_review` regressions reproduce and prevent three defects: an expired receipt reopened by a later pause; allowance surviving a multi-day reward-post outage; and an old registration's refund erasing a new registration's contribution total. Additional cases exercise same-timestamp re-enrollment, renewal without re-enrollment, the exact 72-hour boundary, and resumed/flat reward posts.

`Config.pause_started_at` consumes eight reserved bytes, and `Landlord.first_collection_nonce` consumes eight reserved bytes. Account sizes do not change. The client must use the regenerated IDL; size alone cannot distinguish the old semantics. This is a pre-deployment change, **not** a migration for existing deployed accounts. Any existing deployment would need a separate reviewed migration before using these fields.

The three retained reward marks are pre-post index snapshots. Expired marks are ignored even if posting stops. This may discard some still-recent allowance between mark boundaries, conservatively under-collecting. It limits the age of posted credit, not the age or provenance of an actual wallet payout: the refresher's cumulative total remains trusted, and neither the bound nor the passage of time proves a wallet received PENIS rewards. The worker must apply its separate attribution checks.

The pause logic relies on the seven-day pause cooldown exceeding the 48-hour review extension; another pause cannot overlap that prior extended window. If those constants change, revisit this invariant.
