# zeroclaw-sentinel

**Sentinel** — a self-custody liquidation guardian for
[Kamino](https://kamino.finance) built on
[ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw).

It watches your wallet's lending health every 15 minutes and messages you on
Telegram *before* liquidation risk is real — with the exact repay/add-
collateral math to fix it. It holds **no keys, signs nothing, sends
nothing**: custody tier T0 by construction.

```
┌────────────┐   cron SOP    ┌──────────────────┐   HTTPS GET   ┌────────────┐
│ ZeroClaw   │──────────────▶│ lending_health    │──────────────▶│ Kamino API │
│ daemon     │  every 15m    │ (WASM, wasip2)    │  public data  └────────────┘
│ + Telegram │◀──────────────│ health/LTV/action │
└────────────┘  alert if     └──────────────────┘
   ▲            WARN+
   └─ you, executing fixes in your own wallet
```

## Layout

- `plugins/lending-health/` — the WASM tool plugin (wit/v0 `tool-plugin`
  world). Pure math core (`src/health.rs`) + tolerant API parser
  (`src/parse.rs`) are native-tested; component glue in `src/lib.rs`.
- `sentinel/sops/lending-watch/` — the deterministic monitoring SOP
  (cron + classify + alert, coalescing admission).
- `WRITEUP.md` — custody tier, threat model, reproduction steps.
- `VIDEO_SCRIPT.md` — demo script.
- `BUILD_NOTES.md` — build/integration state (working notes).

## Quick start

See **Reproduce it** in [WRITEUP.md](WRITEUP.md). Short version: source-build
ZeroClaw with `--features plugins-wasm,plugins-wasm-cranelift`, `cargo build
--release --target wasm32-wasip2` in `plugins/lending-health`, `zeroclaw
plugin install`, copy the SOP, run the daemon.

## What v2 added (Aug 2026)

- **Names the token.** Alerts now say *"repay $2,000 of USDC"*, not "repay
  $2,000 of debt". The per-reserve breakdown only exists in the obligation's
  raw on-chain `state` (the API's top-level position maps come back empty), so
  the parser decodes it and maps reserve addresses to symbols via the market's
  `reserves/metrics` feed.
- **An independent second opinion on freshness.** The plugin asks a
  *configurable Solana RPC* for the head slot and reports how far behind it the
  on-chain snapshot is (`onchain_snapshot_age_min`), plus the lending
  program's own stale flag. A stale account paired with a silent "SAFE" is the
  false-safe this exists to prevent. If the RPC is unreachable the claim is
  simply omitted — never invented.
- **Honest value sourcing.** USD figures come from the API's live
  recomputation; token composition comes from the last on-chain refresh. The
  report says which is which rather than blending them.

Both enrichment calls fail soft: if the reserve feed or the RPC is down, the
health check still returns correct numbers, just with less detail.

## Status

Plugin compiles clean to `wasm32-wasip2`; **17/17 native tests pass**.
Verified end to end against a live $33.7M Kamino obligation (dSOL + hSOL
collateral, USDC debt, 30.65% LTV vs 55% liquidation threshold, on-chain
snapshot 84 days stale — correctly flagged). See BUILD_NOTES.md.

MIT licensed.
