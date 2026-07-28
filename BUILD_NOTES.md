# zeroclaw-sentinel — build state & spec capture

Superteam Zeroclaw bounty submission. **Deadline: Aug 6, 11:59pm ET**
(2026-08-07T02:59:59Z). $5K pool, 7 paid slots, 62+ submissions as of Jul 27.

## The submission (what judges receive)
A showcase post in #solana-bounty on ZeroClaw Discord containing:
1. Real running use case — real ZeroClaw agent on a real channel doing a real
   Solana job, GitHub repo linked
2. Video ≤3 min — no slides; terminal + phone only
3. Write-up: what/who/features/custody tier + threat model + reproducible
   config/SOPs/skills links
Plus the Superteam Earn form: demo video link (required), one-pager
(optional), supporting material (required).
Judging: use case 30% / safety+custody 25% / craft 20% / reproducibility 15%
/ showcase 10%. Tiebreak: build-in-public logs on X.
DISQUALIFIERS: trading/sniper bots (BANNED), concepts/slideware, plugin
without use case, thin RPC wrappers padded into WASM, raw key without caps.

## Our concept: "Sentinel" — self-custody liquidation guardian (Kamino)
ZeroClaw agent that watches a wallet's Kamino lending obligations, computes
health/LTV/distance-to-liquidation, alerts on Telegram *before* liquidation
with an exact action plan (repay X of token Y / add Z collateral). Custody
tier T0→T1: read-only monitoring; stretch goal = unsigned repay tx w/ durable
nonce (T1 Build, never holds keys).

Layering (scored — least code preferred):
- Tier 1 (stock): Telegram channel, cron polling SOPs, memory, thresholds.
- Tier 3 (one justified WASM plugin): `lending_health` tool — fetches
  obligations + reserve params, does real math (aggregate LTV, health %,
  per-token price-drop-to-liquidation solve), emits ≤200-token compact JSON.
  Not a thin wrapper: the value is the computation + shaping.

## Spec traps to honor (each = points)
- Pyth Core DEPRECATES Jul 31 — use Pyth w/ API key or Switchboard Crossbar;
  never demo an unauthenticated Hermes endpoint. (Prices only needed for the
  what-if solve; Kamino API values may suffice for v1 — decide + document.)
- Durable nonce for any prepared unsigned tx (rent ~0.0015 SOL,
  AdvanceNonceAccount first ix). Stretch goal territory.
- Tool output ~200 tokens max — judges inspect returns.
- RPC/API keys via `config_read` only; support user-supplied RPC URL.
- Cron polling, not webhooks.
- Prompt-injection test transcript in write-up (alert text is attacker
  surface: obligation/token names could carry injection — test it).
- wit/v0 is experimental — wit copied into crate at plugins/lending-health/wit.

## Plugin contract (from docs/writing-a-tool-plugin.md — verified)
- world tool-plugin: import logging; export plugin-info; export tool.
- tool = name() / description() / parameters-schema() (JSON string, model's
  whole view) / execute(json args) -> result<tool-result{success,output,
  error}, string>. `success:false` for bad input; `Err` only for broken state.
- Stateless: fresh store per call. Config via `__config` injected map
  (flat string->string), requires manifest permission `config_read`; empty
  map MUST produce safe defaults. HTTP needs `http_client` permission + waki
  (0.5.1, json feature), blocking, no sockets.
- Pure logic in plain module (native `cargo test`); glue under
  `#[cfg(target_family="wasm")]` with wit_bindgen::generate!({path:"wit/v0",
  world:"tool-plugin", features:["plugins-wit-v0"]}). Log via imported
  logging interface (log_record), never wasi:logging.
- manifest.toml: name=lending-health (lowercase slug), version, wasm_path=
  lending_health.wasm, capabilities=["tool"], permissions=["http_client",
  "config_read"].
- Build: cargo build --release --target wasm32-wasip2 → target/wasm32-wasip2/
  release/lending_health.wasm (underscores). Install: ~/.zeroclaw/plugins/
  lending-health/{manifest.toml,lending_health.wasm}; zeroclaw plugin install;
  plugins.enabled=true; verify zeroclaw plugin list.
- Host must be source-built: cargo build --release --features
  plugins-wasm,plugins-wasm-cranelift (RUNNING in background, repo at
  Projects/zeroclaw). Judges score against source-built host exactly.

## Kamino API (probed live Jul 27)
- GET api.kamino.finance/kamino-market → [{lendingMarket:
  "7u3HeHxYDLhnCoErrtycNokbQYbWGzLs6JSDqGAv5PfF" (primary), ...}]
- GET .../kamino-market/{market}/reserves/metrics → [{reserve, liquidityToken,
  maxLtv:"0.45", borrowApy, totalSupplyUsd, ...}] (no liquidation LTV here —
  expect it in full reserves endpoint or obligation refreshedStats)
- GET .../kamino-market/{market}/users/{wallet}/obligations → [] for empty
  wallet (200). Need a real obligation fixture: find a public whale wallet
  with klend positions to capture response shape (expect refreshedStats:
  loanToValue, liquidationLtv, userTotalDeposit, userTotalBorrow, positions).

## Task list
- [x] Rust 1.97.1 + wasm32-wasip2 installed; repos cloned; crate scaffolded
- [x] Host source build DONE (26m50s, zeroclaw 0.8.3, MSYS2 mingw fix for
      dlltool; binary at Projects/zeroclaw/target/release/zeroclaw.exe)
- [x] Plugin INSTALLED + discovered: `zeroclaw plugin list/info` show
      lending-health v0.1.0, [Tool], [HttpClient, ConfigRead]; dist/ dir
      pattern works; config entry seeded
- [x] SOP VALIDATED: sop.sops_dir=C:\Users\chris\.zeroclaw\sops,
      `sop validate lending-watch` ✅, `sop list` shows cron+manual triggers
- [ ] End-to-end agent demo BLOCKED on Chris: `claude setup-token` is an
      interactive browser OAuth (tell Chris: type `! claude setup-token` in
      session), then `zeroclaw quickstart --model-provider anthropic`; plus
      Telegram @BotFather token for the channel. `zeroclaw doctor`: 18 ok,
      2 warn, 2 err (expected pre-quickstart: no provider/channel)
- [ ] Capture real obligation fixture (whale wallet or Chris tiny position) —
      synthetic fixtures in tests for now; parser is alias-tolerant
- [x] src/health.rs: pure math core + native tests (11/11 pass)
- [x] src/parse.rs tolerant Kamino parser
- [x] src/lib.rs: component glue (per redact-text reference)
- [x] Compile to wasm32-wasip2 clean → 366KB lending_health.wasm (Jul 27)
- [x] manifest.toml written
- [ ] manifest.toml + install + `zeroclaw plugin list` shows it
- [x] SOP written: sentinel/sops/lending-watch (cron */15, coalesce,
      classify→alert; validate w/ `zeroclaw sop validate` once host built)
- [x] WRITEUP.md drafted (custody/threat model/repro; injection transcript TBD)
- [x] VIDEO_SCRIPT.md drafted (2:50 shot list)
- [x] README.md drafted
- [ ] Agent config: Telegram channel + quickstart (needs bot token — CHRIS:
      5 min with @BotFather). NOTE: LLM auth can use Claude subscription via
      `claude setup-token` + `zeroclaw quickstart --model-provider anthropic`
      — no new API cost, and Agentic grant reimburses the subscription
- [ ] Live demo run on a funded test position (tiny real Kamino position on
      Chris's wallet OR monitor a public whale wallet read-only)
- [ ] Write-up: custody tier, threat model, prompt-injection transcript
- [ ] Video ≤3 min (CHRIS records; script prepared)
- [ ] Discord showcase post + Earn form submit (CHRIS accounts)
- [ ] Stretch: prepare_repay unsigned-tx plugin w/ durable nonce

## Chris-gated items for this bounty
Superteam Earn account (submit), Discord account (showcase post), Telegram
bot token via @BotFather (demo channel), video recording, optional tiny
Kamino position (~$20 SOL) for an authentic demo.
