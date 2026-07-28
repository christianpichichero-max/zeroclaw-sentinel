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

## Status

Built for the Superteam × ZeroClaw Solana bounty (Aug 2026). Plugin compiles
clean to `wasm32-wasip2`; 11/11 native tests pass. Integration + demo in
progress — see BUILD_NOTES.md.

MIT licensed.
