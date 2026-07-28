# Demo video — shot-by-shot (target 2:30, hard cap 3:00)

Rules from the spec: no slides, terminal + phone only, ≥720p, no AI
voiceover, 2–4 min or auto-reject (ETHGlobal rule; Zeroclaw says ≤3 — stay
≤3:00).

Setup before recording: daemon running, Telegram chat open on phone, a
wallet with a real (small) Kamino borrow position configured, terminal font
large.

| t | Shot | Say (natural, unscripted feel) |
|---|---|---|
| 0:00–0:15 | Terminal: `zeroclaw plugin list` showing `lending-health` | "This is Sentinel — a liquidation guardian for Kamino running on ZeroClaw. One WASM plugin, read-only, holds zero keys." |
| 0:15–0:45 | Terminal: agent chat — ask "check my kamino health" → tool call visible → compact JSON report | "The plugin computes what actually matters: I'm at 62% LTV, liquidation at 75%, my collateral can drop 17% before I'm in trouble — and it solves the fix: repay $840 or add $1,700." |
| 0:45–1:15 | Terminal: `cat sops/lending-watch/SOP.md`, then `zeroclaw sop validate lending-watch` | "Monitoring isn't vibes — it's a deterministic SOP: cron every 15 minutes, classify, alert only when it matters. Auditable runs, coalescing so it never double-fires." |
| 1:15–1:50 | Phone: Telegram alert arriving (trigger by temporarily lowering warn threshold in config) | "Here's the alert on my phone: severity, the numbers, the exact action. And the important part — Sentinel never signs anything. I execute in my own wallet." |
| 1:50–2:15 | Terminal: manifest.toml on screen | "Safety is structural: the manifest grants exactly two permissions — HTTP out and its own config. No keys exist anywhere in this system. Worst-case compromise is a wrong report." |
| 2:15–2:35 | Terminal: prompt-injection test — mock response with hostile token symbol, agent relays it as data | "Even if the data source tries to inject instructions, it's carried as structured data — the agent reports it, doesn't obey it." |
| 2:35–2:50 | Terminal: `cargo test` green (11 tests) + repo URL on screen | "Eleven native tests on the math core, full repro steps in the repo. Sentinel — sleep through the dip, not through the liquidation." |
