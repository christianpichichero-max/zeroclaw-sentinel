# Sentinel tools

Three small Python files, standard library only, no install step. They exist so
the two contracts Sentinel cares about — *read health honestly* and *never hand
over an unverified transaction* — can be exercised from any MCP client without
building the Rust toolchain first.

The WASM plugins in `plugins/` are the production path. These are the
reproducible one. They agree on the maths, the field names, and the refusals.

| File | What it does |
|---|---|
| `kamino_health.py` | Reads a wallet's Kamino lending health and prints JSON: LTV, liquidation LTV, distance to liquidation, per-token composition, repay amount, and how old the on-chain snapshot is. |
| `prepare_repay.py` | Asks Kamino to build an **unsigned** repay, then independently decodes the returned instructions and refuses anything that is not the repay that was approved. |
| `test_verify.py` | 18 tests for the verifier, mostly adversarial. Runs offline. |
| `mock_kamino.py` | Local stand-in for the Kamino API, for testing without network. |

## Reading health

```
python tools/kamino_health.py --wallet <address>
```

Verified live against a real obligation on the primary market:

```
python tools/kamino_health.py --wallet 9Z6qhmZ2AHWMSBSM4LmA1WrCAefs2dkr1pHnkW3vmg8z \
    --target-ltv 0.35 --warn-drop 50 --crit-drop 20 --stale-max-min 180
```

```json
{
  "status": "WARN",
  "deposits_usd": 35111.08,
  "borrows_usd": 14447.63,
  "ltv_pct": 42.03,
  "liquidation_ltv_pct": 80.0,
  "drop_to_liquidation_pct": 47.46,
  "collateral_tokens": ["cbBTC"],
  "debt_tokens": ["USDC", "SOL", "ETH"],
  "repay_token": "USDC",
  "repay_usd_to_target": 2158.75,
  "snapshot_age_min": 13.5,
  "program_flags_stale": true
}
```

Thresholds are operator configuration, not opinions held by the tool.
`--target-ltv` is the LTV a repayment should restore; `--warn-drop` and
`--crit-drop` are how much room is left, in percent the collateral can fall
before liquidation.

Two details worth stating plainly, because they are the difference between a
health check and a false sense of one:

**Dollar values come from the API's refreshed stats, never from the on-chain
snapshot.** A Kamino obligation only refreshes when someone touches it, so
`state` can be hours or months behind while looking perfectly well-formed. It is
used here for composition — which tokens — and never for current value.

**Freshness is established independently.** The tool reads head slot from a
Solana RPC and compares it to the obligation's own last-update slot. Past
`--stale-max-min` the status becomes `UNKNOWN` with a reason. A stale snapshot
reported as `SAFE` is the exact failure a liquidation watch exists to prevent.

## Preparing a repay

```
python tools/prepare_repay.py --wallet <owner> --reserve <reserve> --amount 25 --decimals 6
```

This calls `POST /ktx/klend/repay-instructions` rather than `/ktx/klend/repay`.
The difference matters: `/repay` bakes in a live blockhash that expires in about
ninety seconds, and an approval that has to survive a human reading their email
needs a transaction whose lifetime the caller controls. The instructions
endpoint returns no blockhash and no fee payer, which is what makes durable-nonce
signing possible.

Nothing is signed and nothing is submitted. The output is an unsigned
instruction set for the owner to sign in their own wallet.

### What the verifier checks

An external API builds these instructions. Nothing about that API forces it to
return a repay — a compromised or simply mistaken endpoint could hand back a
token transfer, a different amount, or a different owner. So the bytes are
decoded here and checked:

1. **No foreign programs.** Every instruction belongs to Kamino Lending or
   ComputeBudget. This is the check that catches "the API returned a transfer".
2. **A real repay instruction exists**, identified by its anchor discriminator
   (`74aed54cb435d290` for v2, `91b20de14cf09348` for v1) — found by scanning
   backwards, because the number of `refresh_reserve` instructions varies with
   the obligation and a fixed index would be wrong.
3. **The amount is the approved amount**, decoded as a little-endian u64 out of
   the instruction data.
4. **The signer is the requested wallet**, and **the reserve appears** in the
   instruction's accounts.

Any failure returns `DO NOT SIGN` and no artifact.

### A gap the tests found

The original check accepted the amount if it matched the request at *any* power
of ten. That is weaker than it looks. An approval of 25 USDC is 25,000,000 base
units at 6 decimals — but 2,500,000,000 base units, a hundred times as much, is
also exactly 25 of an eight-decimal token, and so it passed.

Pass `--decimals` and the scale is pinned to the token, so there is exactly one
correct number of base units. The decimals are always available after a health
read, so there is no reason to omit them. When they are omitted the verdict says
`"amount_scale_pinned_to_token": false` rather than quietly implying the amount
was fully checked.

```
python tools/test_verify.py     # 18 tests, offline
```
