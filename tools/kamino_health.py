#!/usr/bin/env python3
"""Read Kamino lending health for a wallet and report it as JSON.

This is a standalone, dependency-free port of the `lending-health` WASM plugin
in this repository. It exists so the health contract can be exercised from any
MCP client without building the Rust toolchain first: the plugin is the
production path, this is the reproducible one.

The two implementations agree on the parts that matter:

  * Current dollar values come from the API's refreshed stats, never from the
    on-chain snapshot, which can be months behind.
  * Per-token composition comes from `state.deposits` / `state.borrows`, whose
    `marketValueSf` values are scaled fractions with a 2^60 factor. The
    top-level deposits/borrows maps come back empty on live obligations.
  * Freshness is established independently, by reading head slot from a Solana
    RPC and comparing it to the obligation's own last-update slot. A snapshot
    that cannot be aged is UNKNOWN, never SAFE.

Usage:
    python kamino_health.py --wallet <address>
    python kamino_health.py --wallet <address> --target-ltv 0.5 --warn-drop 25

Exit codes: 0 report produced, 2 nothing to report, 3 upstream failure.
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request

API_BASE = "https://api.kamino.finance"
RPC_URL = "https://api.mainnet-beta.solana.com"
PRIMARY_MARKET = "7u3HeHxYDLhnCoErrtycNokbQYbWGzLs6JSDqGAv5PfF"

#: Kamino encodes fractional values as scaled fractions with a 2^60 factor.
SCALED_FRACTION = float(1 << 60)

#: Solana targets 400ms per slot. Good enough to turn a slot gap into minutes.
SECONDS_PER_SLOT = 0.4

TIMEOUT = 20


# --------------------------------------------------------------------------
# transport
# --------------------------------------------------------------------------


#: The API sits behind a WAF that rejects the default urllib agent string.
USER_AGENT = "sentinel-lending-health/1.0 (+https://github.com/christianpichichero-max/zeroclaw-sentinel)"


def _get(url: str) -> str:
    req = urllib.request.Request(
        url, headers={"accept": "application/json", "user-agent": USER_AGENT}
    )
    with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
        return resp.read().decode("utf-8")


def _post_json(url: str, payload: dict) -> dict:
    body = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=body,
        headers={"content-type": "application/json", "user-agent": USER_AGENT},
    )
    with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
        return json.loads(resp.read().decode("utf-8"))


# --------------------------------------------------------------------------
# tolerant field access
# --------------------------------------------------------------------------


def num(value) -> float:
    """Kamino returns numbers as numbers or as decimal strings. Accept both."""
    if isinstance(value, bool):
        return 0.0
    if isinstance(value, (int, float)):
        return float(value)
    if isinstance(value, str):
        try:
            return float(value.strip())
        except ValueError:
            return 0.0
    return 0.0


def pick(obj: dict, *keys: str) -> float:
    """First present key wins. Mirrors the serde aliases on the Rust side."""
    for key in keys:
        if isinstance(obj, dict) and key in obj:
            return num(obj[key])
    return 0.0


def positions_of(container) -> list:
    """deposits/borrows arrive as a list, or as an object keyed by reserve."""
    if isinstance(container, list):
        return container
    if isinstance(container, dict):
        return list(container.values())
    return []


# --------------------------------------------------------------------------
# upstream reads
# --------------------------------------------------------------------------


def reserve_symbols(api_base: str, market: str) -> dict:
    """reserve address -> token symbol. Failure here degrades, never fails."""
    try:
        rows = json.loads(_get(f"{api_base}/kamino-market/{market}/reserves/metrics"))
    except (urllib.error.URLError, ValueError, OSError):
        return {}
    out = {}
    if isinstance(rows, list):
        for row in rows:
            if not isinstance(row, dict):
                continue
            reserve = row.get("reserve")
            token = row.get("liquidityToken")
            if reserve and token:
                out[reserve] = token
    return out


def head_slot(rpc_url: str):
    """Current chain head, or None if the RPC cannot be reached."""
    try:
        resp = _post_json(
            rpc_url,
            {"jsonrpc": "2.0", "id": 1, "method": "getSlot", "params": [{"commitment": "confirmed"}]},
        )
    except (urllib.error.URLError, ValueError, OSError):
        return None
    slot = resp.get("result")
    return int(slot) if isinstance(slot, (int, float)) else None


# --------------------------------------------------------------------------
# composition and assessment
# --------------------------------------------------------------------------


def compose(state_positions, symbols: dict) -> list:
    """Per-token breakdown, largest first. Proportions only - values are stale."""
    out = []
    for entry in state_positions or []:
        if not isinstance(entry, dict):
            continue
        reserve = entry.get("reserve") or entry.get("depositReserve") or entry.get("borrowReserve") or ""
        usd = pick(entry, "marketValueSf") / SCALED_FRACTION
        if not reserve or reserve.startswith("111111") or usd <= 0:
            continue
        label = symbols.get(reserve) or (reserve[:6] + ".." if len(reserve) > 6 else reserve)
        out.append({"token": label, "usd": round(usd, 2)})
    out.sort(key=lambda row: row["usd"], reverse=True)
    return out


def assess(deposits_usd, borrows_usd, ltv, liq_ltv, target_ltv, warn_drop, crit_drop) -> dict:
    """Status plus the two ways out: repay debt, or add collateral."""
    if borrows_usd <= 0:
        return {"status": "NO_DEBT", "drop_to_liquidation_pct": None,
                "repay_usd_to_target": None, "add_collateral_usd": None}

    if liq_ltv <= 0 or ltv <= 0 or deposits_usd <= 0:
        return {"status": "UNKNOWN", "drop_to_liquidation_pct": None,
                "repay_usd_to_target": None, "add_collateral_usd": None}

    # LTV = B / V. With borrows fixed, liquidation arrives when V falls to
    # B / liq_ltv - a fractional drop of 1 - ltv/liq_ltv.
    drop = max(0.0, 1.0 - ltv / liq_ltv) * 100.0
    repay = max(0.0, borrows_usd - target_ltv * deposits_usd)
    add = max(0.0, borrows_usd / target_ltv - deposits_usd)

    if ltv >= liq_ltv:
        status = "LIQUIDATABLE"
    elif drop <= crit_drop:
        status = "CRITICAL"
    elif drop <= warn_drop:
        status = "WARN"
    else:
        status = "SAFE"

    return {
        "status": status,
        "drop_to_liquidation_pct": round(drop, 2),
        "repay_usd_to_target": round(repay, 2),
        "add_collateral_usd": round(add, 2),
    }


def read_obligation(raw: dict, symbols: dict, head, args) -> dict:
    stats = raw.get("refreshedStats") or {}
    state = raw.get("state") or {}
    last_update = state.get("lastUpdate") or {}

    deposits_usd = pick(stats, "userTotalDeposit", "userTotalDepositUsd", "totalDepositUsd")
    borrows_usd = pick(stats, "userTotalBorrow", "userTotalBorrowUsd", "totalBorrowUsd")
    ltv = pick(stats, "loanToValue", "ltv")
    liq_ltv = pick(stats, "liquidationLtv", "unhealthyLoanToValue")

    snapshot_slot = int(pick(last_update, "slot"))
    program_stale = pick(last_update, "stale") != 0

    if head and snapshot_slot:
        age_min = round(max(0, head - snapshot_slot) * SECONDS_PER_SLOT / 60.0, 1)
    else:
        age_min = None

    report = assess(
        deposits_usd, borrows_usd, ltv, liq_ltv,
        args.target_ltv, args.warn_drop, args.crit_drop,
    )

    # A reading we cannot age is a failure to observe, not a clean bill of
    # health. Say so rather than letting a stale snapshot read as SAFE.
    if age_min is None or (args.stale_max_min and age_min > args.stale_max_min):
        report["status"] = "UNKNOWN"
        report["unknown_reason"] = (
            "snapshot age could not be established"
            if age_min is None
            else f"snapshot is {age_min} min old, past the {args.stale_max_min} min limit"
        )

    debt = compose(state.get("borrows"), symbols)
    collateral = compose(state.get("deposits"), symbols)

    return {
        "obligation": raw.get("obligationAddress") or raw.get("obligation") or raw.get("address") or "",
        "deposits_usd": round(deposits_usd, 2),
        "borrows_usd": round(borrows_usd, 2),
        "ltv_pct": round(ltv * 100, 2),
        "liquidation_ltv_pct": round(liq_ltv * 100, 2),
        "collateral_tokens": [row["token"] for row in collateral[:3]],
        "debt_tokens": [row["token"] for row in debt[:3]],
        # The token to name in an alert. An amount without its token is not
        # actionable, so the caller should refuse to send one.
        "repay_token": debt[0]["token"] if debt else None,
        "snapshot_slot": snapshot_slot,
        "snapshot_age_min": age_min,
        "program_flags_stale": program_stale,
        **report,
    }


# --------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(description="Kamino lending health, as JSON.")
    parser.add_argument("--wallet", required=True, help="Owner wallet address.")
    parser.add_argument("--market", default=PRIMARY_MARKET)
    parser.add_argument("--api-base", default=API_BASE)
    parser.add_argument("--rpc-url", default=RPC_URL, help="Solana RPC, read only, for head slot.")
    parser.add_argument("--target-ltv", type=float, default=0.50,
                        help="LTV the repay amount should restore. Default 0.50.")
    parser.add_argument("--warn-drop", type=float, default=25.0,
                        help="Collateral drop %% to liquidation that triggers WARN.")
    parser.add_argument("--crit-drop", type=float, default=10.0,
                        help="Collateral drop %% to liquidation that triggers CRITICAL.")
    parser.add_argument("--stale-max-min", type=float, default=0.0,
                        help="Snapshot age in minutes past which health is UNKNOWN. 0 disables.")
    args = parser.parse_args()

    symbols = reserve_symbols(args.api_base, args.market)
    head = head_slot(args.rpc_url)

    url = f"{args.api_base}/kamino-market/{args.market}/users/{args.wallet}/obligations"
    try:
        raw = json.loads(_get(url))
    except urllib.error.HTTPError as exc:
        print(json.dumps({"error": f"Kamino API returned {exc.code}"}), file=sys.stderr)
        return 3
    except (urllib.error.URLError, ValueError, OSError) as exc:
        print(json.dumps({"error": f"Kamino API unreachable: {exc}"}), file=sys.stderr)
        return 3

    if not isinstance(raw, list) or not raw:
        print(json.dumps({"wallet": args.wallet, "obligations": [],
                          "note": "no obligations on this market"}, indent=2))
        return 2

    obligations = [read_obligation(item, symbols, head, args) for item in raw if isinstance(item, dict)]

    # Worst first, so a reader who stops at the first entry still sees the
    # thing that needs attention.
    rank = {"LIQUIDATABLE": 0, "CRITICAL": 1, "WARN": 2, "UNKNOWN": 3, "SAFE": 4, "NO_DEBT": 5}
    obligations.sort(key=lambda o: (rank.get(o["status"], 9), -o["borrows_usd"]))

    print(json.dumps({
        "wallet": args.wallet,
        "market": args.market,
        "head_slot": head,
        "target_ltv_pct": round(args.target_ltv * 100, 2),
        "obligations": obligations,
    }, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
