use std::collections::HashMap;

use lending_health::health::{assess, render, Config, Obligation, Status};
use lending_health::parse::parse_obligations;

fn cfg() -> Config {
    Config::from_section(&HashMap::new())
}

fn ob(deposits: f64, borrows: f64, ltv: f64, liq: f64) -> Obligation {
    Obligation {
        address: "ObLiGaTiOn111".into(),
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
    let out = render("WaLLeT", "MaRKeT", &[], &cfg());
    assert!(out.contains("NO_POSITIONS"));
}

#[test]
fn render_is_compact_and_reports_worst_status() {
    let obs = vec![
        ob(10_000.0, 3_000.0, 0.30, 0.75),
        ob(10_000.0, 7_000.0, 0.70, 0.75),
    ];
    let out = render("WaLLeT", "MaRKeT", &obs, &cfg());
    assert!(out.contains("\"status\":\"CRITICAL\""), "worst-first status: {out}");
    assert!(out.contains("repay $2000"), "action plan present: {out}");
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
    let obs = parse_obligations(body).unwrap();
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
    let obs = parse_obligations(body).unwrap();
    let o = &obs[0];
    assert!((o.ltv - 0.55).abs() < 1e-9, "percent normalized: {}", o.ltv);
    assert!((o.liq_ltv - 0.80).abs() < 1e-9);
    assert!((o.deposits_usd - 100.0).abs() < 1e-9, "fallback sum from positions");
}

#[test]
fn garbage_body_is_a_clean_error() {
    assert!(parse_obligations("<html>oops</html>").is_err());
}
