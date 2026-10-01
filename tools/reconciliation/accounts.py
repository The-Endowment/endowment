"""Read-only decoders for the PR #3 account layout (0f7cb96), not a signer.

Amounts stay decimal strings in saved JSON. These observations are never
collection permissions or evidence that a wallet received a particular reward.
"""

import base64
import hashlib

ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
TOKEN_PROGRAMS = {
    "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
    "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb",
}


def b58encode(data):
    number, result = int.from_bytes(data, "big"), ""
    while number:
        number, digit = divmod(number, 58)
        result = ALPHABET[digit] + result
    return "1" * (len(data) - len(data.lstrip(b"\0"))) + result


def public_key(value):
    if not isinstance(value, str) or not 32 <= len(value) <= 44:
        raise ValueError("Invalid public key")
    number = 0
    for char in value:
        if char not in ALPHABET:
            raise ValueError("Invalid public key")
        number = number * 58 + ALPHABET.index(char)
    size = (number.bit_length() + 7) // 8 + len(value) - len(value.lstrip("1"))
    if size != 32:
        raise ValueError("Public key must encode 32 bytes")
    return value


def discriminator(name):
    return hashlib.sha256(f"account:{name}".encode()).digest()[:8]


def account_bytes(account, program, name):
    if not account or account.get("owner") != program or account.get("executable"):
        raise ValueError(f"Missing or foreign {name} account")
    encoded = account.get("data")
    if not isinstance(encoded, list) or len(encoded) != 2 or encoded[1] != "base64":
        raise ValueError(f"Expected base64 {name} account")
    data = base64.b64decode(encoded[0], validate=True)
    if data[:8] != discriminator(name) or len(data) < 9 or data[8] != 3:
        raise ValueError(f"Unsupported {name} layout; this reader requires PR #3 version 3")
    return data


class Reader:
    def __init__(self, data):
        self.data, self.offset = data, 9

    def take(self, length):
        end = self.offset + length
        if end > len(self.data):
            raise ValueError("Truncated account")
        result, self.offset = self.data[self.offset:end], end
        return result

    def key(self):
        return b58encode(self.take(32))

    def number(self, length=8, signed=False):
        return int.from_bytes(self.take(length), "little", signed=signed)


def decode_config(account, program):
    data = account_bytes(account, program, "Config")
    reader = Reader(data)
    for _ in range(4):
        reader.key()  # creator, admin, pending admin, guardian
    return {
        "coin_mint": reader.key(),
        "pump_mint": reader.key(),
        "data_sha256": hashlib.sha256(data).hexdigest(),
    }


def decode_landlord(address, account, program, config):
    data = account_bytes(account, program, "Landlord")
    reader = Reader(data)
    if reader.key() != config:
        raise ValueError("Landlord belongs to another config")
    result = {
        "landlord": public_key(address), "wallet": reader.key(),
        "pump_account": reader.key(), "coin_account": reader.key(),
        "baseline_raw": str(reader.number()),
        # Upstream calls this total_contributed. It is the cumulative gross
        # amount DEBITED during this registration, not the wallet's balance.
        "collected_gross_raw": str(reader.number()),
        "registered_at": reader.number(signed=True),
        "last_sweep_at": reader.number(signed=True),
    }
    reader.number(1)  # bump
    result["joined_round"] = str(reader.number())
    result["counted_round"] = str(reader.number())
    result["counted_coin_raw"] = str(reader.number())
    result["count_snapshot_raw"] = str(reader.number())
    result["data_sha256"] = hashlib.sha256(data).hexdigest()
    return result


def token_observation(account, wallet, mint):
    """Missing/changed accounts are unknown, never invented zero balances."""
    if account is None:
        return {"status": "missing", "amount_raw": None}
    if account.get("owner") not in TOKEN_PROGRAMS or account.get("executable"):
        return {"status": "wrong_program", "amount_raw": None}
    data = account.get("data")
    if not isinstance(data, dict) or not isinstance(data.get("parsed"), dict):
        return {"status": "unparsed_account", "amount_raw": None}
    parsed = data["parsed"]
    info = parsed.get("info", {})
    if parsed.get("type") != "account" or info.get("owner") != wallet or info.get("mint") != mint:
        return {"status": "ownership_or_mint_changed", "amount_raw": None}
    amount = info.get("tokenAmount", {}).get("amount")
    if (not isinstance(amount, str) or not amount.isascii() or not amount.isdigit()
            or len(amount) > 20 or int(amount) > 2**64 - 1):
        raise ValueError("Malformed token amount")
    return {
        "status": "observed", "amount_raw": amount,
        "state": info.get("state"), "delegate": info.get("delegate"),
        "delegated_amount_raw": info.get("delegatedAmount", {}).get("amount", "0"),
    }
