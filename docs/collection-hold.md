# Collection: hold, review and reclaim

How dividend (PUMP) gets from a landlord's account to the endowment's buyback vault. Based on the design in PR #4.

## The path of one collection

1. **Collect.** The collector signs `sweep(nonce, report)`. The report names the exact amount, the account's exact balance, the landlord's consent epoch, and an expiry at most 60 seconds away. The contract then takes the smallest of:
   - the report's amount;
   - what sits above the landlord's baseline;
   - the landlord's reward allowance (what its counted coin earned, 1.0×, carried over about three days);
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
- **Every refund** raises the landlord's baseline by what came back, so it is never collected again, and takes it out of the contribution totals.

## Timing

| | |
|---|---|
| Report valid for | at most 60 seconds |
| Hold before review and release | 24 hours |
| Refund-only after | 72 hours |
| A pause | blocks release, never a refund; the 48-hour review window starts again when it ends |

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
