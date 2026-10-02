"""Offline regression tests; no wallet, network, or deployed program needed."""

import base64
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import urllib.error

from accounts import b58encode, discriminator, public_key, token_observation
from snapshot import Rpc, capture, save_once

PROGRAM = b58encode(bytes([1]) * 32)
CONFIG = b58encode(bytes([2]) * 32)
WALLET = b58encode(bytes([3]) * 32)
COIN = b58encode(bytes([4]) * 32)
PUMP = b58encode(bytes([5]) * 32)
COIN_ACCOUNT = b58encode(bytes([6]) * 32)
PUMP_ACCOUNT = b58encode(bytes([7]) * 32)
LANDLORD = b58encode(bytes([8]) * 32)
TOKEN_PROGRAM = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"


def encoded(data):
    return {"owner": PROGRAM, "executable": False, "data": [base64.b64encode(data).decode(), "base64"]}


def config_account():
    # Independent Borsh fixture of the stable Config header in state.rs.
    return encoded(discriminator("Config") + bytes([3]) + bytes(128) + bytes([4]) * 32 + bytes([5]) * 32)


def landlord_account(collected=25, version=3):
    # Config, owner, PUMP ATA, coin ATA; then baseline/contributed/timestamps.
    data = discriminator("Landlord") + bytes([version])
    data += b"".join(bytes([n]) * 32 for n in [2, 3, 7, 6])
    data += b"".join(n.to_bytes(8, "little") for n in [12, collected, 1000, 2000])
    data += bytes([254])
    data += b"".join(n.to_bytes(8, "little") for n in [1, 2, 100, 200])
    return encoded(data)


def token(mint, amount="123"):
    return {"owner": TOKEN_PROGRAM, "executable": False, "data": {"parsed": {
        "type": "account", "info": {
            "owner": WALLET, "mint": mint, "state": "initialized",
            "tokenAmount": {"amount": amount},
        },
    }}}


class FixtureRpc:
    def __init__(self):
        self.calls = 0
        self.methods = []
        self.closing_account = landlord_account()
        self.coin = token(COIN, str(2**60 + 1))
        self.pump = token(PUMP)
        self.opening_account = landlord_account()
        self.token_slot = 102
        self.short_batch = False

    def call(self, method, params):
        self.calls += 1
        self.methods.append(method)
        self.assert_read(params)
        if method == "getAccountInfo":
            return {"context": {"slot": 100}, "value": config_account()}
        if method == "getProgramAccounts":
            ending = self.methods.count(method) == 2
            account = self.closing_account if ending else self.opening_account
            return {"context": {"slot": 103 if ending else 101}, "value": [
                {"pubkey": LANDLORD, "account": account},
            ]}
        if method == "getMultipleAccounts":
            assert params[0] == [COIN_ACCOUNT, PUMP_ACCOUNT]
            values = [self.coin] if self.short_batch else [self.coin, self.pump]
            return {"context": {"slot": self.token_slot}, "value": values}
        raise AssertionError("Unexpected method")

    @staticmethod
    def assert_read(params):
        assert params[-1]["commitment"] == "finalized"


class SnapshotTests(unittest.TestCase):
    def test_keeps_large_integers_and_distinguishes_live_and_counted_holdings(self):
        result = capture(FixtureRpc(), PROGRAM, CONFIG)
        row = result["wallets"][0]
        self.assertEqual(row["coin"]["amount_raw"], str(2**60 + 1))
        self.assertEqual(row["counted_coin_raw"], "100")
        self.assertEqual(row["collected_gross_raw"], "25")
        self.assertEqual(result["token_batches"][0]["slot"], 102)
        self.assertEqual(result["enrollment_slot"], 101)
        self.assertTrue(result["enrollment_unchanged_during_scan"])

    def test_closed_token_account_is_unknown_not_zero(self):
        rpc = FixtureRpc()
        rpc.coin = None
        row = capture(rpc, PROGRAM, CONFIG)["wallets"][0]
        self.assertEqual(row["coin"], {"status": "missing", "amount_raw": None})

    def test_account_reassigned_to_another_owner_is_not_used(self):
        value = token(COIN)
        value["data"]["parsed"]["info"]["owner"] = CONFIG
        self.assertIsNone(token_observation(value, WALLET, COIN)["amount_raw"])

    def test_wrong_mint_and_foreign_program_are_not_used(self):
        self.assertIsNone(token_observation(token(PUMP), WALLET, COIN)["amount_raw"])
        value = token(COIN)
        value["owner"] = PROGRAM
        self.assertEqual(token_observation(value, WALLET, COIN)["status"], "wrong_program")

    def test_unparsed_accounts_remain_unknown(self):
        value = token(COIN)
        value["data"] = ["AAAA", "base64"]
        self.assertEqual(token_observation(value, WALLET, COIN)["status"], "unparsed_account")

    def test_malformed_token_amount_fails_closed(self):
        for amount in [1.5, "-1", "1.5", str(2**64), "9" * 100]:
            with self.assertRaisesRegex(ValueError, "Malformed"):
                token_observation(token(COIN, amount), WALLET, COIN)

    def test_many_wallets_are_batched_with_separate_observation_slots(self):
        rows = [{
            "landlord": LANDLORD, "wallet": WALLET, "coin_account": COIN_ACCOUNT,
            "pump_account": PUMP_ACCOUNT, "data_sha256": str(i),
        } for i in range(51)]

        class BatchRpc(FixtureRpc):
            sizes = []

            def call(self, method, params):
                if method != "getMultipleAccounts":
                    return super().call(method, params)
                self.calls += 1
                self.sizes.append(len(params[0]))
                return {"context": {"slot": 101 + len(self.sizes)}, "value": [
                    self.coin if i % 2 == 0 else self.pump for i in range(len(params[0]))
                ]}

        rpc = BatchRpc()
        with patch("snapshot.enrollments", side_effect=[(101, rows), (104, rows)]):
            result = capture(rpc, PROGRAM, CONFIG)
        self.assertEqual(rpc.sizes, [100, 2])
        self.assertEqual([x["slot"] for x in result["token_batches"]], [102, 103])
        self.assertEqual(result["wallets"][50]["token_batch"], 1)

    def test_mid_scan_collection_is_reported_as_changed(self):
        rpc = FixtureRpc()
        rpc.closing_account = landlord_account(collected=30)
        result = capture(rpc, PROGRAM, CONFIG)
        self.assertFalse(result["enrollment_unchanged_during_scan"])
        self.assertEqual(result["wallets"][0]["collected_gross_raw"], "25")

    def test_partial_response_stops_the_run(self):
        rpc = FixtureRpc()
        rpc.short_batch = True
        with self.assertRaisesRegex(ValueError, "Incomplete"):
            capture(rpc, PROGRAM, CONFIG)

    def test_slot_regression_stops_the_run(self):
        rpc = FixtureRpc()
        rpc.token_slot = 99
        with self.assertRaisesRegex(ValueError, "regressing"):
            capture(rpc, PROGRAM, CONFIG)

    def test_other_layout_is_rejected(self):
        rpc = FixtureRpc()
        rpc.opening_account = landlord_account(version=4)
        with self.assertRaisesRegex(ValueError, "Unsupported"):
            capture(rpc, PROGRAM, CONFIG)

    def test_snapshot_publication_never_overwrites_a_previous_day(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "day.json"
            save_once(path, {"first": 1})
            with self.assertRaises(FileExistsError):
                save_once(path, {"second": 2})
            self.assertEqual(json.loads(path.read_text()), {"first": 1})
            self.assertEqual([p.name for p in Path(directory).iterdir()], ["day.json"])

    def test_read_only_rpc_rejects_signing_or_broadcast_methods(self):
        with self.assertRaisesRegex(ValueError, "Only snapshot"):
            Rpc("https://example.invalid").call("sendTransaction", [])

    def test_rpc_failure_does_not_disclose_provider_credentials(self):
        with patch("urllib.request.urlopen", side_effect=urllib.error.URLError("secret-api-key")):
            with self.assertRaises(RuntimeError) as result:
                Rpc("https://example.invalid/?api-key=secret-api-key").call("getAccountInfo", [])
        self.assertNotIn("secret", str(result.exception))

    def test_rejects_invalid_public_keys(self):
        for value in ["", "0" * 32, "1" * 31, "1" * 33, None]:
            with self.assertRaises(ValueError):
                public_key(value)
        self.assertEqual(public_key("1" * 32), "1" * 32)


if __name__ == "__main__":
    unittest.main()
