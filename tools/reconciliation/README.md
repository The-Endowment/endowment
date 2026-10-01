# Wallet checkpoints and a refundable collection hold

This branch now implements the holding contract described in
[the draft design and trust model](../../docs/refundable-collection.md), plus this
read-only checkpoint tool. The contract and companion worker/UI are not deployed.
No collection policy has been finalized. The old PR comment's statement that an
all-Stonk-PUMP policy was agreed should not be treated as a new requirement.

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

## How these checkpoints relate to release

The implemented contract uses a separate holding account and individual receipt
timers. Its full behavior, defaults, evidence requirements, recovery paths and
remaining launch gates are documented in
[refundable-collection.md](../../docs/refundable-collection.md).

The companion website worker performs wallet-history reconciliation. Midnight
PENIS/PUMP checkpoints and aggregate API deltas remain sanity checks only. They
cannot create an allowance, approve a receipt, or authorize a release.
