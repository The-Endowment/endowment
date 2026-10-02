# Daily wallet checkpoints

A read-only monitoring tool. Once a day it records each enrolled wallet's $PENIS, PUMP and collection totals, so collections can be compared with each wallet's estimated share. It never signs or sends a transaction.

```sh
python3 tools/reconciliation/snapshot.py \
  --program PROGRAM_PUBLIC_KEY \
  --config CONFIG_PUBLIC_KEY \
  --output-dir /absolute/path/to/durable/checkpoints
python3 -m unittest discover -s tools/reconciliation -p 'test_*.py' -v
```

Python 3's standard library is enough. Put the RPC URL in `SOLANA_RPC_URL`.

- It discovers enrolled landlord records and reads their token accounts in batches of up to 100 per RPC call: `3 + ceil(2N / 100)` requests for N wallets.
- It records the finalized slot and time of each batch. Batches are not one atomic midnight read.
- Missing accounts, changed owners or failed reads are recorded as unknown, never as zero.
- Daily files are written atomically and never overwritten.

These checkpoints are for monitoring and the daily digest. The on-chain reward allowance already holds each wallet to its share before a collection happens; the hold and review are described in [docs/collection-hold.md](../../docs/collection-hold.md).
