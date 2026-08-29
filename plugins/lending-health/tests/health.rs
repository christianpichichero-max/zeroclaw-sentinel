use std::collections::HashMap;

use lending_health::health::{assess, render, Config, Obligation, Status};
use lending_health::parse::{parse_obligations, ReserveMap};

fn cfg() -> Config {
    Config::from_section(&HashMap::new())
}

fn no_reserves() -> ReserveMap {
    ReserveMap::new()
}

fn ob(deposits: f64, borrows: f64, ltv: f64, liq: f64) -> Obligation {
    Obligation {
        address: "ObLiGaTiOn111".into(),
        collateral_tokens: vec![("SOL".into(), deposits)],
        debt_tokens: vec![("USDC".into(), borrows)],
        last_update_slot: 0,
        onchain_stale: false,
        deposits_usd: deposits,
        borrows_usd: borrows,
        ltv,
        liq_ltv: liq,
        top_deposits: vec![("SOL".into(), deposits)],
        top_borrows: vec![("USDC".into(), borrows)],
    }
}

#[test]
fn empty_config_gives_safe_defaults() {
    let c = cfg();
    assert_eq!(c.api_base, "https://api.kamino.finance");
    assert!(c.target_ltv > 0.0 && c.target_ltv < 1.0);
    assert!(c.crit_drop_pct < c.warn_drop_pct);
}

#[test]
fn no_debt_is_not_at_risk() {
    let a = assess(&ob(1000.0, 0.0, 0.0, 0.75), &cfg());
    assert_eq!(a.status, Status::NoDebt);
    assert!(a.price_drop_to_liq_pct.is_none());
}

#[test]
fn healthy_position_is_safe_with_correct_drop() {
    // ltv 0.30, liq 0.75 -> collateral can drop 60% before liquidation.
    let a = assess(&ob(10_000.0, 3_000.0, 0.30, 0.75), &cfg());
    assert_eq!(a.status, Status::Safe);
    let drop = a.price_drop_to_liq_pct.unwrap();
    assert!((drop - 60.0).abs() < 0.01, "drop was {drop}");
}

#[test]
fn near_liquidation_is_critical_with_action_math() {
    // ltv 0.70, liq 0.75 -> only a 6.67% drop of collateral to liquidation.
    let a = assess(&ob(10_000.0, 7_000.0, 0.70, 0.75), &cfg());
    assert_eq!(a.status, Status::Critical);
    // Repay to 50% target: 7000 - 0.5*10000 = 2000.
    assert!((a.repay_usd_to_target.unwrap() - 2000.0).abs() < 0.01);
    // Add collateral to 50%: 7000/0.5 - 10000 = 4000.
    assert!((a.add_collateral_usd.unwrap() - 4000.0).abs() < 0.01);
}

#[test]
fn past_liq_ltv_is_liquidatable() {
    let a = assess(&ob(10_000.0, 8_000.0, 0.80, 0.75), &cfg());
    assert_eq!(a.status, Status::Liquidatable);
}

#[test]
fn missing_liq_ltv_is_unknown_not_crash() {
    let a = assess(&ob(10_000.0, 5_000.0, 0.50, 0.0), &cfg());
    assert_eq!(a.status, Status::Unknown);
}

#[test]
fn render_no_positions_is_calm() {
    let out = render("WaLLeT", "MaRKeT", &[], &cfg(), None);
    assert!(out.contains("NO_POSITIONS"));
}

#[test]
fn render_is_compact_and_reports_worst_status() {
    let obs = vec![
        ob(10_000.0, 3_000.0, 0.30, 0.75),
        ob(10_000.0, 7_000.0, 0.70, 0.75),
    ];
    let out = render("WaLLeT", "MaRKeT", &obs, &cfg(), None);
    assert!(out.contains("\"status\":\"CRITICAL\""), "worst-first status: {out}");
    assert!(out.contains("repay $2000 of USDC"), "action names the token: {out}");
    // ~200-token budget: keep the whole report under ~900 chars for 2 positions.
    assert!(out.len() < 900, "render too verbose: {} chars", out.len());
}

#[test]
fn parses_documented_api_shape_with_string_numbers() {
    let body = r#"[{
        "obligationAddress": "9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin",
        "refreshedStats": {
            "userTotalDeposit": "12000.5",
            "userTotalBorrow": "6000.25",
            "loanToValue": "0.50",
            "liquidationLtv": "0.75"
        },
        "deposits": [
            {"symbol": "JitoSOL", "marketValueRefreshed": "8000.5"},
            {"symbol": "SOL", "marketValueRefreshed": "4000.0"}
        ],
        "borrows": [
            {"symbol": "USDC", "marketValueRefreshed": "6000.25"}
        ]
    }]"#;
    let obs = parse_obligations(body, &no_reserves()).unwrap();
    assert_eq!(obs.len(), 1);
    let o = &obs[0];
    assert!((o.deposits_usd - 12000.5).abs() < 0.01);
    assert!((o.ltv - 0.50).abs() < 1e-9);
    assert!((o.liq_ltv - 0.75).abs() < 1e-9);
    assert_eq!(o.top_deposits[0].0, "JitoSOL");
}

#[test]
fn parses_percent_style_ltv_and_missing_stats() {
    let body = r#"[{
        "address": "AbC",
        "refreshedStats": {"loanToValue": "55", "liquidationLtv": "80"},
        "deposits": [{"liquidityToken": "SOL", "amountUsd": 100.0}],
        "borrows": [{"liquidityToken": "USDC", "amountUsd": 55.0}]
    }]"#;
    let obs = parse_obligations(body, &no_reserves()).unwrap();
    let o = &obs[0];
    assert!((o.ltv - 0.55).abs() < 1e-9, "percent normalized: {}", o.ltv);
    assert!((o.liq_ltv - 0.80).abs() < 1e-9);
    assert!((o.deposits_usd - 100.0).abs() < 1e-9, "fallback sum from positions");
}

#[test]
fn garbage_body_is_a_clean_error() {
    assert!(parse_obligations("<html>oops</html>", &no_reserves()).is_err());
}

#[test]
fn parses_real_live_api_fixture_with_map_style_positions() {
    // Captured 2026-07-28 from the live obligations endpoint: deposits/borrows
    // arrive as (possibly empty) objects, not arrays; stats carry the LTV.
    let body = include_str!("fixtures_whale.json");
    let obs = parse_obligations(body, &no_reserves()).unwrap();
    assert_eq!(obs.len(), 1);
    let o = &obs[0];
    assert!(o.deposits_usd > 4_000_000.0, "deposits {}", o.deposits_usd);
    assert!(o.borrows_usd > 2_500_000.0);
    assert!((o.ltv - 0.756).abs() < 0.01, "ltv {}", o.ltv);
    assert!((o.liq_ltv - 0.90).abs() < 1e-9);
    let out = render("whale", "main", &obs, &cfg(), None);
    // drop-to-liq = 1 - .756/.9 = 16% -> WARN at default 25/10 thresholds.
    assert!(out.contains("\"status\":\"WARN\""), "expected WARN: {out}");
}

// ---- v2: composition, reserve mapping, and independent freshness ----

#[test]
fn reserve_map_parses_metrics_feed() {
    let body = r#"[
        {"reserve":"D6q6wuQSrifJKZYpR1M6","liquidityToken":"USDC","maxLtv":"0.8"},
        {"reserve":"StGKGcLQoTsWzQ1tFY2b","liquidityToken":"dSOL"},
        {"reserve":"NoSymbolHere","liquidityToken":""}
    ]"#;
    let m = lending_health::parse::parse_reserves(body);
    assert_eq!(m.get("D6q6wuQSrifJKZYpR1M6").map(String::as_str), Some("USDC"));
    assert_eq!(m.get("StGKGcLQoTsWzQ1tFY2b").map(String::as_str), Some("dSOL"));
    // Rows without a symbol are dropped rather than producing empty labels.
    assert!(!m.contains_key("NoSymbolHere"));
}

#[test]
fn malformed_reserve_feed_degrades_quietly() {
    // A broken enrichment feed must never take the health check down with it.
    assert!(lending_health::parse::parse_reserves("not json").is_empty());
}

#[test]
fn live_fixture_yields_token_composition_from_onchain_state() {
    // Captured live 2026-08-29. The top-level deposits/borrows maps are empty;
    // the per-reserve breakdown only exists under `state`, so this is the
    // path that makes an alert say "repay your USDC" instead of "repay debt".
    let body = include_str!("fixtures_live2.json");
    let mut reserves = ReserveMap::new();
    reserves.insert("D6q6wuQSrifJKZYpR1M6mA9NKQuSIsyi7RbNo1Cj9Zn".into(), "USDC".into());
    let obs = parse_obligations(body, &reserves).unwrap();
    let o = &obs[0];
    assert!(o.borrows_usd > 0.0, "live fixture should carry debt");
    assert!(!o.debt_tokens.is_empty(), "debt composition recovered from state");
    assert!(!o.collateral_tokens.is_empty(), "collateral composition recovered");
    // The 2^60 scaled-fraction decode must land in a sane dollar range, not
    // astronomically wrong: within an order of magnitude of the fresh total.
    let stale_debt: f64 = o.debt_tokens.iter().map(|p| p.1).sum();
    assert!(
        stale_debt > o.borrows_usd * 0.1 && stale_debt < o.borrows_usd * 10.0,
        "scaled-fraction decode out of range: stale {stale_debt} vs fresh {}",
        o.borrows_usd
    );
}

#[test]
fn onchain_staleness_is_surfaced_not_hidden() {
    let body = include_str!("fixtures_live2.json");
    let obs = parse_obligations(body, &no_reserves()).unwrap();
    let o = &obs[0];
    assert!(o.last_update_slot > 0, "last update slot parsed");
    assert!(o.onchain_stale, "fixture's account carries the stale flag");

    // With a head slot far ahead of the snapshot, the report must say so.
    let head = o.last_update_slot + 15_000_000;
    let out = render("w", "m", &obs, &cfg(), Some(head));
    assert!(
        out.contains("onchain_snapshot_age_min"),
        "age must be reported when head slot is known: {out}"
    );
    assert!(out.contains("\"note\""), "stale flag must be explained: {out}");

    // Without an RPC second opinion we simply omit the claim - we never
    // invent a freshness we could not verify.
    let out_no_rpc = render("w", "m", &obs, &cfg(), None);
    assert!(!out_no_rpc.contains("onchain_snapshot_age_min"));
}

#[test]
fn rpc_url_is_configurable_with_a_safe_default() {
    assert!(cfg().rpc_url.starts_with("https://"));
    let mut m = HashMap::new();
    m.insert("rpc_url".to_string(), "https://my-own-node.example/rpc".to_string());
    assert_eq!(Config::from_section(&m).rpc_url, "https://my-own-node.example/rpc");
}
