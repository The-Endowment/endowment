# Wallet checkpoints and a refundable collection hold

This is a review prototype based on endowment PR #3, commit `0f7cb96`.
It adds a read-only wallet checkpoint collector and a proposed contract design.
It does **not** change the deployed program, implement escrow, approve a
collection, or establish that the allowance model protects purchased PUMP.
No collection policy has been finalized. The old PR comment's statement that
an all-Stonk-PUMP policy was agreed should not be treated as a new requirement.

## What runs now

Python 3's standard library is sufficient. Supply a reviewed version-3 config
and program address, and put your provider URL in `SOLANA_RPC_URL`:

```sh
python3 tools/reconciliation/snapshot.py \
  --program PROGRAM_PUBLIC_KEY \
  --config CONFIG_PUBLIC_KEY \
  --output-dir /absolute/path/to/durable/checkpoints
python3 -m unittest discover -s tools/reconciliation -p 'test_*.py' -v
```

The collector discovers enrolled Landlord records and batches the registered
PENIS and PUMP token accounts, up to 100 accounts per RPC call. It records
actual finalized slots and observation times; batches are not an atomic
midnight read. It stores counted holdings separately from observed holdings,
gross cumulative debits separately from wallet balances, and delegation details
without asserting that registration means current consent.

Missing accounts, changed owners, bad data, or a failed read are not converted
to zero. A second enrollment read detects changes during the scan. Daily files
are published atomically and never overwritten. The collector cannot submit
transactions, and it does not load private keys or log provider URLs. It needs
durable storage. There is no production schedule configured by this change.

For N wallets it normally makes `3 + ceil(2N / 100)` RPC requests: one config
read, two enrollment scans, and batched token reads. Provider credits can differ
by method. This excludes reward API calls, transaction history, retries, and
storage. Only the enrolled token accounts are covered, not other accounts that
the same wallet may own. The public program is not assumed to be deployed.

These are operational checkpoints. Same balances at both endpoints do not
prove that nothing changed between them. A wallet can spend rewards and buy
PUMP again while still matching its estimated daily contribution. A collection
counter can reset on re-enrollment. Daily comparisons must handle missing days,
identity/counter resets and lifecycle changes as inconclusive, and must retain
transaction evidence before any release decision. This collector alone does
not implement the proposed per-wallet reconciliation or a release decision.

## Proposed holding contract (not implemented here)

The proposed flow is collect, check, then release. The minimum hold is **24
elapsed hours after each collection**, independent of calendar labels. Only
eligible rewards should be collected in the first place. A holding period does
not authorize intentionally collecting purchased PUMP.

1. A collection creates an immutable receipt containing the endowment, source
   wallet, token mint, amount debited, amount actually received, collection
   time, consent generation, and a unique collection identifier.
2. Pending PUMP is held under a separate authority. Buybacks can spend only the
   released treasury balance. There must be no permissionless legacy path that
   skips the hold.
3. **Holder choice confirmed:** the holder can reclaim their own pending
   contribution before release. Returning funds must continue to work during
   pauses, retirement, goal completion, revoked delegation, and deregistration.
   A closed token account needs a safe replacement account owned by that holder.
4. A review records cleared/refundable amounts and supporting evidence. The
   midnight checkpoint and Stonk aggregate delta can flag outliers; neither is
   proof of a wallet's unspent eligible reward receipts. Uncertain observations
   cannot automatically become approval. Partial mistakes need partial refunds.
5. Release requires both the elapsed hold and an explicit valid review. It can
   only move cleared funds to this endowment's spendable PUMP vault. Refunds can
   only return recorded amounts to the original holder. No actor gets a general
   treasury withdrawal permission. All paths prevent double settlement.
6. Unresolved receipts have a finite refund deadline. After it, anyone can crank
   a refund to the original holder; the service also needs to fund and submit
   those transactions. Time passing does not execute a Solana instruction.
7. A refund disables that holder's future collection until explicit re-consent.
   Returning PUMP must not replenish a stale allowance or let the next sweep
   take it again. This protection must survive deregistration/re-enrollment and
   is a necessary change to the current allowance architecture.
8. Only permanently owned PENIS in the designated vault counts toward 200M.
   Direct donations count. On goal completion, stop new collection and make
   outstanding pending contributions refundable; do not silently commit them
   after the funding goal is already met.

Release would still rely on whoever checks historical rewards. A signed review
is an attestation, not on-chain proof that the money was eligible. Reusing the
collection key for approval means one compromise can falsify both steps. An
independent reviewer reduces that shared failure but adds key management. A
compromised upgrade authority could bypass either design. These tradeoffs must
be explicit rather than hidden behind the word "reconciled."

## Decisions and implementation sequence

The holder's reclaim right is decided. Still to decide: PENIS-only versus other
Stonk PUMP rewards, approval authority and rotation, refund timeout, treatment
of late payouts, token-fee policy, and what evidence is sufficient to clear an
amount. Midnight balance equality is never enough by itself.

Next, exercise this reader against a supported test deployment and add a
read-only comparison report with actual reward/spending history. Review the
escrow state machine and authority model before connecting it to collection.
Then implement contract settlement, worker recovery, and wallet reclaim UI
together, with explicit new consent. Do not deploy a partial combination.

Required integration tests include premature release, unknown/incomplete
history, malicious approval, wrong recipients/mints/configs, double refunds,
partial refunds, refund-then-resweep, changing balances within one day, pending
funds surviving opt-out/account closure, expiry when the worker is offline,
goal completion by direct donation, and pending funds being inaccessible to
every buyback path. Existing upstream allowance limitations remain in scope.
