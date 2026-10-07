"""Capture enrolled-wallet checkpoints. No keys, signing, or sendTransaction.

Run near midnight UTC. RPC batches have their own finalized slots; this is
NOT an atomic midnight snapshot or a historical-balance query.
"""

import argparse
import datetime as dt
import json
import os
from pathlib import Path
import tempfile
import urllib.error
import urllib.request

from accounts import b58encode, decode_config, decode_landlord, discriminator, public_key, token_observation


def now():
    return dt.datetime.now(dt.timezone.utc).isoformat()


class Rpc:
    def __init__(self, url):
        if not url.startswith("https://") and not url.startswith(("http://127.0.0.1:", "http://localhost:")):
            raise ValueError("RPC must use HTTPS (except local test servers)")
        self.url, self.calls = url, 0

    def call(self, method, params):
        if method not in {"getAccountInfo", "getProgramAccounts", "getMultipleAccounts"}:
            raise ValueError("Only snapshot read methods are permitted")
        self.calls += 1
        request = urllib.request.Request(self.url, data=json.dumps({
            "jsonrpc": "2.0", "id": self.calls, "method": method, "params": params,
        }).encode(), headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                result = json.load(response)
        except (urllib.error.URLError, TimeoutError, ValueError) as exc:
            # Never print a provider URL containing an API key.
            raise RuntimeError(f"RPC {method} failed ({type(exc).__name__})") from None
        if result.get("error") or "result" not in result:
            raise RuntimeError(f"RPC {method} returned an error")
        return result["result"]


def checked_slot(response, minimum):
    slot = response.get("context", {}).get("slot")
    if type(slot) is not int or slot < minimum:
        raise ValueError("RPC returned a missing or regressing slot")
    return slot


def enrollments(rpc, program, config, minimum):
    response = rpc.call("getProgramAccounts", [program, {
        "encoding": "base64", "commitment": "finalized", "withContext": True,
        "minContextSlot": minimum,
        "filters": [
            {"memcmp": {"offset": 0, "bytes": b58encode(discriminator("Landlord"))}},
            {"memcmp": {"offset": 9, "bytes": config}},
        ],
    }])
    slot = checked_slot(response, minimum)
    rows = [decode_landlord(x["pubkey"], x["account"], program, config) for x in response["value"]]
    rows.sort(key=lambda x: x["wallet"])
    if len({x["wallet"] for x in rows}) != len(rows):
        raise ValueError("Duplicate enrolled wallet")
    return slot, rows


def capture(rpc, program, config):
    public_key(program)
    public_key(config)
    start = now()
    response = rpc.call("getAccountInfo", [config, {"encoding": "base64", "commitment": "finalized"}])
    start_slot = checked_slot(response, 0)
    settings = decode_config(response["value"], program)
    enrollment_slot, rows = enrollments(rpc, program, config, start_slot)
    batches = []
    minimum = enrollment_slot
    # Two known token accounts per enrolled wallet: at most 100 per call.
    for first in range(0, len(rows), 50):
        group = rows[first:first + 50]
        addresses = [x[key] for x in group for key in ("coin_account", "pump_account")]
        requested = now()
        response = rpc.call("getMultipleAccounts", [addresses, {
            "encoding": "jsonParsed", "commitment": "finalized", "minContextSlot": minimum,
        }])
        minimum = checked_slot(response, minimum)
        values = response["value"]
        if len(values) != len(addresses):
            raise ValueError("Incomplete token-account batch")
        batch_index = len(batches)
        batches.append({"requested_at": requested, "received_at": now(), "slot": minimum})
        for i, row in enumerate(group):
            row["token_batch"] = batch_index
            row["coin"] = token_observation(values[2 * i], row["wallet"], settings["coin_mint"])
            row["pump"] = token_observation(values[2 * i + 1], row["wallet"], settings["pump_mint"])
    # Preserve the fact that enrollment/counters moved during the scan; do not
    # combine later counters with earlier balances as if read atomically.
    end_slot, ending = enrollments(rpc, program, config, minimum)
    same = [(x["landlord"], x["data_sha256"]) for x in rows] == [
        (x["landlord"], x["data_sha256"]) for x in ending
    ]
    return {
        "schema": 2, "kind": "observational-wallet-checkpoint", "source_layout": "Config-v4-Landlord-v3",
        "program": program, "config": config, **settings,
        "started_at": start, "finished_at": now(),
        "config_slot": start_slot, "enrollment_slot": enrollment_slot, "end_slot": end_slot,
        "enrollment_unchanged_during_scan": same,
        "rpc_calls": rpc.calls, "token_batches": batches, "wallets": rows,
        "limitations": [
            "Only enrolled token accounts are read, not every account owned by each wallet.",
            "Endpoint balances do not reveal intraday trades, reward provenance, or spent rewards.",
            "Registration is not proof of current consent or a valid delegation to the endowment.",
            "Contribution counters are net of this registration's refunds and reset on re-enrollment.",
            "Counter deltas cannot separate gross collections from refunds; use receipt/events for that.",
            "This file cannot authorize collection, refund, approval, or release.",
        ],
    }


def save_once(path, value):
    """Atomic publication, no replacing a prior daily checkpoint, durable file."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=".checkpoint-", dir=path.parent)
    try:
        with os.fdopen(descriptor, "w") as stream:
            json.dump(value, stream, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.link(temporary, path)  # Fails atomically if the destination exists.
        directory = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        os.unlink(temporary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--program", required=True)
    parser.add_argument("--config", required=True)
    parser.add_argument("--rpc-env", default="SOLANA_RPC_URL", help="Environment variable containing the RPC URL")
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    endpoint = os.environ.get(args.rpc_env)
    if not endpoint:
        parser.error("RPC URL environment variable is not set")
    try:
        result = capture(Rpc(endpoint), args.program, args.config)
        day = result["started_at"][:10]
        path = args.output_dir / args.program / args.config / f"{day}.json"
        save_once(path, result)
    except (ValueError, RuntimeError, OSError) as exc:
        parser.exit(1, f"Snapshot failed: {exc}\n")
    print(json.dumps({"path": str(path), "wallets": len(result["wallets"]), "rpc_calls": result["rpc_calls"]}))


if __name__ == "__main__":
    main()
