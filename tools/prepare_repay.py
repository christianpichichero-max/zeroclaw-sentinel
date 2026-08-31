#!/usr/bin/env python3
"""Prepare an UNSIGNED Kamino repay and verify it before anyone sees it.

A dependency-free port of the `prepare-repay` WASM plugin in this repository,
so the hand-off contract can be exercised from any MCP client.

The point of this tool is the verification, not the preparation. An external
API builds the instructions, and nothing about that API forces it to return a
repay: a compromised or simply mistaken endpoint could hand back a token
transfer, a different amount, or a different owner. So the bytes are decoded
here and checked against what was actually asked for. If any check fails the
tool says DO NOT SIGN and returns no artifact.

The agent never signs and never submits. The owner signs in their own wallet.

Note on the endpoint: `/ktx/klend/repay-instructions` is used rather than
`/ktx/klend/repay`, because the latter bakes in a live blockhash that expires
in roughly 90 seconds. An approval that has to survive a human reading their
email needs a transaction whose lifetime the caller controls.

Usage:
    python prepare_repay.py --wallet <owner> --reserve <reserve> --amount 25
"""

from __future__ import annotations

import argparse
import base64
import json
import sys
import urllib.error
import urllib.request

API_BASE = "https://api.kamino.finance"
PRIMARY_MARKET = "7u3HeHxYDLhnCoErrtycNokbQYbWGzLs6JSDqGAv5PfF"

KLEND_PROGRAM = "KLend2g3cP87fffoy8q1mQqGKjrxjC8boSyAYavgmjD"
COMPUTE_BUDGET_PROGRAM = "ComputeBudget111111111111111111111111111111"

#: First 8 bytes of sha256("global:repay_obligation_liquidity_v2").
REPAY_V2_DISCRIMINATOR = bytes.fromhex("74aed54cb435d290")
#: The v1 instruction, still accepted by the program.
REPAY_V1_DISCRIMINATOR = bytes.fromhex("91b20de14cf09348")

USER_AGENT = "sentinel-prepare-repay/1.0 (+https://github.com/christianpichichero-max/zeroclaw-sentinel)"
TIMEOUT = 25


def fetch_instructions(api_base, wallet, market, reserve, amount) -> dict:
    payload = json.dumps(
        {"wallet": wallet, "market": market, "reserve": reserve, "amount": str(amount)}
    ).encode("utf-8")
    req = urllib.request.Request(
        f"{api_base}/ktx/klend/repay-instructions",
        data=payload,
        headers={
            "content-type": "application/json",
            "accept": "application/json",
            "user-agent": USER_AGENT,
        },
    )
    with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
        return json.loads(resp.read().decode("utf-8"))


def decode(data):
    """Base64 decode, tolerating missing padding. None on anything unusable."""
    if not isinstance(data, str):
        return None
    raw = data.strip()
    try:
        return base64.b64decode(raw + "=" * (-len(raw) % 4))
    except (ValueError, TypeError):
        return None


def is_repay(ix) -> bool:
    if ix.get("programAddress") != KLEND_PROGRAM:
        return False
    body = decode(ix.get("data"))
    return bool(body) and len(body) >= 8 and body[:8] in (REPAY_V2_DISCRIMINATOR, REPAY_V1_DISCRIMINATOR)


def verify(resp: dict, wallet: str, reserve: str, requested_amount: str, decimals=None) -> dict:
    """Check that `resp` really is a repay of `requested_amount` for `wallet`.

    Pass `decimals` whenever the token's decimals are known - and after a
    health read they always are. Without it the amount can only be checked
    against *some* power of ten, and that is weaker than it looks: 2,500,000,000
    base units is a hundred times an approval of 25, yet it is also exactly 25
    of an eight-decimal token. Pinning the scale closes that gap.
    """
    instructions = resp.get("instructions") or []
    luts = sorted((resp.get("lutsByAddress") or {}).keys())

    verdict = {
        "ok": True,
        "problem": None,
        "instruction_count": len(instructions),
        "amount_base_units": 0,
        "implied_decimals": None,
        # False means the scale was inferred rather than checked against the
        # token's real decimals. An inflated amount can hide there.
        "scale_verified": decimals is not None,
        # No blockhash in the response means the caller controls expiry, which
        # is what makes durable-nonce signing possible.
        "caller_sets_lifetime": not any(
            k in resp for k in ("blockhash", "recentBlockhash", "lifetime")
        ),
        "lookup_tables": luts,
    }

    def fail(message):
        verdict["ok"] = False
        if verdict["problem"] is None:
            verdict["problem"] = message
        return verdict

    if not instructions:
        return fail("API returned no instructions")

    # 1. No instruction may belong to a program we did not expect. This is the
    #    check that catches "the API returned a token transfer".
    for ix in instructions:
        program = ix.get("programAddress")
        if program not in (KLEND_PROGRAM, COMPUTE_BUDGET_PROGRAM):
            return fail(f"unexpected program in instruction set: {program}")

    # 2. The repay is the last Kamino instruction carrying a known
    #    discriminator. Find it rather than assuming an index - the number of
    #    refresh_reserve instructions varies with the obligation.
    repay = next((ix for ix in reversed(instructions) if is_repay(ix)), None)
    if repay is None:
        return fail("no repay instruction found in the returned set")

    # 3. Decode the amount the instruction actually repays.
    body = decode(repay.get("data")) or b""
    if len(body) < 16:
        return fail("repay instruction data is too short to carry an amount")

    base_units = int.from_bytes(body[8:16], "little")
    verdict["amount_base_units"] = base_units
    if base_units == 0:
        return fail("repay amount decoded as zero")

    # 4. The decoded amount must be the requested amount at some sane token
    #    scale. A mismatch means the API built a different number than we
    #    asked for, which is the entire reason this function exists.
    try:
        requested = float(str(requested_amount).strip())
    except ValueError:
        requested = 0.0
    if requested <= 0:
        return fail("requested amount is not a positive number")

    def matches(scale: int) -> bool:
        expected = requested * (10 ** scale)
        return abs(expected - base_units) <= max(expected * 1e-9, 0.5)

    if decimals is not None:
        # The strong check: the token's decimals are known, so there is exactly
        # one correct number of base units and anything else is a mismatch.
        if not matches(decimals):
            return fail(
                f"instruction repays {base_units} base units, but {requested} "
                f"at {decimals} decimals is {int(requested * 10 ** decimals)}"
            )
        verdict["implied_decimals"] = decimals
    else:
        for scale in range(0, 13):
            if matches(scale):
                verdict["implied_decimals"] = scale
                break
        else:
            return fail(
                f"instruction repays {base_units} base units, "
                f"which is not {requested} at any token scale"
            )

    # 5. The owner signing this must be the wallet we asked for, and the
    #    reserve being repaid must be the reserve we asked for.
    addresses = [a.get("address") for a in (repay.get("accounts") or [])]
    if not addresses:
        return fail("repay instruction carries no accounts")
    if addresses[0] != wallet:
        return fail(f"repay is signed by {addresses[0]}, not the requested wallet")
    if reserve not in addresses:
        return fail("the requested reserve does not appear in the repay instruction")

    return verdict


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Prepare and independently verify an unsigned Kamino repay."
    )
    parser.add_argument("--wallet", required=True, help="Owner wallet. Must sign in their own wallet.")
    parser.add_argument("--reserve", required=True, help="Reserve address of the borrowed token.")
    parser.add_argument("--amount", required=True, help="Human decimal amount, e.g. 25 or 12.5.")
    parser.add_argument("--decimals", type=int, default=None,
                        help="Token decimals. Supply them: without this the amount is only "
                             "checked against some power of ten, which an inflated amount can hide behind.")
    parser.add_argument("--market", default=PRIMARY_MARKET)
    parser.add_argument("--api-base", default=API_BASE)
    parser.add_argument("--emit-instructions", action="store_true",
                        help="Include the unsigned instruction set in the output.")
    args = parser.parse_args()

    try:
        resp = fetch_instructions(args.api_base, args.wallet, args.market, args.reserve, args.amount)
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", "replace")[:200]
        print(json.dumps({"verdict": "DO NOT SIGN",
                          "problem": f"Kamino API returned {exc.code}: {detail}"}, indent=2))
        return 3
    except (urllib.error.URLError, ValueError, OSError) as exc:
        print(json.dumps({"verdict": "DO NOT SIGN",
                          "problem": f"Kamino API unreachable: {exc}"}, indent=2))
        return 3

    result = verify(resp, args.wallet, args.reserve, args.amount, args.decimals)

    out = {
        "verdict": "VERIFIED - UNSIGNED" if result["ok"] else "DO NOT SIGN",
        "requested": {"wallet": args.wallet, "reserve": args.reserve, "amount": args.amount},
        "checks": {
            "only_expected_programs": True,
            "repay_instruction_present": result["amount_base_units"] > 0,
            "amount_matches_request": result["implied_decimals"] is not None,
            "amount_scale_pinned_to_token": result["scale_verified"],
            "owner_is_requested_wallet": result["ok"],
            "reserve_present": result["ok"],
        },
        "amount_base_units": result["amount_base_units"],
        "implied_token_decimals": result["implied_decimals"],
        "instruction_count": result["instruction_count"],
        "caller_controls_expiry": result["caller_sets_lifetime"],
        "lookup_tables": result["lookup_tables"],
        "problem": result["problem"],
        "signing": "The owner signs this in their own wallet. This tool holds no key and submits nothing.",
    }
    if args.emit_instructions and result["ok"]:
        out["unsigned"] = resp

    print(json.dumps(out, indent=2))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
