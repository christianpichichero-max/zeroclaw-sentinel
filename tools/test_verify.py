#!/usr/bin/env python3
"""Adversarial tests for the repay verifier. No network: pure fixtures.

Every test here is a way the preparation API could hand back something that is
not the repay we asked for. The verifier's job is to notice. A test that only
proved the happy path would prove nothing worth proving.

Run:  python tools/test_verify.py
"""

from __future__ import annotations

import base64
import os
import struct
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from prepare_repay import (  # noqa: E402
    COMPUTE_BUDGET_PROGRAM,
    KLEND_PROGRAM,
    REPAY_V1_DISCRIMINATOR,
    REPAY_V2_DISCRIMINATOR,
    verify,
)

WALLET = "GJtYKPNYB4MEWttsdQTmYvvweosmFTGokiWdS9oYSsn8"
RESERVE = "D6q6wuQSrifJKZYpR1M8R4YawnLDtDsMmWM1NbBmgJ59"
OTHER = "So11111111111111111111111111111111111111112"
TOKEN_PROGRAM = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"


def repay_data(base_units: int, discriminator: bytes = REPAY_V2_DISCRIMINATOR) -> str:
    """An anchor instruction: 8-byte discriminator then a little-endian u64."""
    return base64.b64encode(discriminator + struct.pack("<Q", base_units)).decode()


def instruction(program, data, accounts):
    return {
        "programAddress": program,
        "data": data,
        "accounts": [{"address": a, "role": "readonly"} for a in accounts],
    }


def response(*instructions, luts=("FGMSBiyVE8TvZcdQnZETAAKw28tkQJ2ccZy6pyp95URb",)):
    return {"instructions": list(instructions), "lutsByAddress": {k: [] for k in luts}}


def good_repay(base_units=25_000_000, owner=WALLET, reserve=RESERVE):
    return instruction(KLEND_PROGRAM, repay_data(base_units), [owner, reserve, OTHER])


def compute_budget():
    """ComputeBudget instructions carry no anchor data. Must be tolerated."""
    return instruction(COMPUTE_BUDGET_PROGRAM, None, [])


class VerifierAcceptsRealRepays(unittest.TestCase):
    def test_accepts_a_correct_repay(self):
        v = verify(response(compute_budget(), good_repay()), WALLET, RESERVE, "25")
        self.assertTrue(v["ok"], v["problem"])
        self.assertEqual(v["amount_base_units"], 25_000_000)
        self.assertEqual(v["implied_decimals"], 6)

    def test_accepts_the_v1_instruction(self):
        ix = instruction(
            KLEND_PROGRAM,
            repay_data(25_000_000, REPAY_V1_DISCRIMINATOR),
            [WALLET, RESERVE],
        )
        self.assertTrue(verify(response(ix), WALLET, RESERVE, "25")["ok"])

    def test_finds_the_repay_after_refresh_instructions(self):
        # The number of refresh_reserve instructions varies with the
        # obligation, so the repay cannot be found at a fixed index.
        refresh = instruction(KLEND_PROGRAM, base64.b64encode(b"\x01" * 24).decode(), [OTHER])
        v = verify(response(compute_budget(), refresh, refresh, good_repay()), WALLET, RESERVE, "25")
        self.assertTrue(v["ok"], v["problem"])
        self.assertEqual(v["instruction_count"], 4)

    def test_reports_caller_controlled_expiry(self):
        # No blockhash in the response is what makes durable-nonce signing
        # possible, and it is worth saying so out loud.
        v = verify(response(good_repay()), WALLET, RESERVE, "25")
        self.assertTrue(v["caller_sets_lifetime"])

    def test_notices_a_baked_in_blockhash(self):
        resp = response(good_repay())
        resp["blockhash"] = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM"
        self.assertFalse(verify(resp, WALLET, RESERVE, "25")["caller_sets_lifetime"])

    def test_decimal_amounts_scale_correctly(self):
        v = verify(response(good_repay(12_500_000)), WALLET, RESERVE, "12.5")
        self.assertTrue(v["ok"], v["problem"])
        self.assertEqual(v["implied_decimals"], 6)


class VerifierRefusesEverythingElse(unittest.TestCase):
    def assertRefused(self, verdict, contains):
        self.assertFalse(verdict["ok"])
        self.assertIn(contains, verdict["problem"])

    def test_refuses_a_foreign_program(self):
        # The headline attack: the API returns a token transfer instead.
        transfer = instruction(TOKEN_PROGRAM, repay_data(25_000_000), [WALLET, OTHER])
        self.assertRefused(
            verify(response(transfer, good_repay()), WALLET, RESERVE, "25"),
            "unexpected program",
        )

    def test_refuses_a_wrong_owner(self):
        ix = good_repay(owner=OTHER)
        self.assertRefused(verify(response(ix), WALLET, RESERVE, "25"), "not the requested wallet")

    def test_refuses_a_wrong_reserve(self):
        ix = instruction(KLEND_PROGRAM, repay_data(25_000_000), [WALLET, OTHER])
        self.assertRefused(
            verify(response(ix), WALLET, RESERVE, "25"),
            "reserve does not appear",
        )

    def test_refuses_an_inflated_amount_when_decimals_are_known(self):
        # Approval was for 25 USDC, which is 25,000,000 base units at 6
        # decimals. The instruction repays a hundred times that. Without the
        # token's decimals this slips through, because 2,500,000,000 is also
        # exactly 25 of an eight-decimal token - so the decimals are the check.
        self.assertRefused(
            verify(response(good_repay(2_500_000_000)), WALLET, RESERVE, "25", decimals=6),
            "at 6 decimals is 25000000",
        )

    def test_inferred_scale_is_reported_as_unverified(self):
        # Same instruction set, no decimals supplied. It passes, and the
        # verdict says plainly that the scale was inferred rather than checked.
        v = verify(response(good_repay(2_500_000_000)), WALLET, RESERVE, "25")
        self.assertTrue(v["ok"])
        self.assertFalse(v["scale_verified"])
        self.assertEqual(v["implied_decimals"], 8)

    def test_refuses_an_amount_off_by_a_non_power_of_ten(self):
        self.assertRefused(
            verify(response(good_repay(37_000_000)), WALLET, RESERVE, "25"),
            "not 25.0 at any token scale",
        )

    def test_refuses_a_zero_amount(self):
        self.assertRefused(verify(response(good_repay(0)), WALLET, RESERVE, "25"), "decoded as zero")

    def test_refuses_when_no_repay_instruction_exists(self):
        refresh = instruction(KLEND_PROGRAM, base64.b64encode(b"\x02" * 24).decode(), [WALLET])
        self.assertRefused(
            verify(response(compute_budget(), refresh), WALLET, RESERVE, "25"),
            "no repay instruction found",
        )

    def test_refuses_an_empty_instruction_set(self):
        self.assertRefused(verify(response(), WALLET, RESERVE, "25"), "no instructions")

    def test_refuses_truncated_instruction_data(self):
        ix = instruction(KLEND_PROGRAM, base64.b64encode(REPAY_V2_DISCRIMINATOR).decode(), [WALLET, RESERVE])
        self.assertRefused(verify(response(ix), WALLET, RESERVE, "25"), "too short")

    def test_refuses_a_repay_with_no_accounts(self):
        ix = instruction(KLEND_PROGRAM, repay_data(25_000_000), [])
        self.assertRefused(verify(response(ix), WALLET, RESERVE, "25"), "carries no accounts")

    def test_refuses_a_non_numeric_requested_amount(self):
        self.assertRefused(
            verify(response(good_repay()), WALLET, RESERVE, "all of it"),
            "not a positive number",
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
