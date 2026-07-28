//! Tolerant parsing of the Kamino API obligations response into normalized
//! `health::Obligation`s. Field names verified against api.kamino.finance;
//! aliases cover the shape variants seen in the klend SDK. No I/O here.

use serde::Deserialize;

use crate::health::Obligation;

/// Number that may arrive as a JSON number or a decimal string ("0.4521").
#[derive(Deserialize, Default)]
#[serde(untagged)]
pub enum Num {
    F(f64),
    S(String),
    #[default]
    Missing,
}

impl Num {
    fn val(&self) -> f64 {
        match self {
            Num::F(v) if v.is_finite() => *v,
            Num::S(s) => s.trim().parse::<f64>().unwrap_or(0.0),
            _ => 0.0,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct ApiObligation {
    #[serde(alias = "obligation", alias = "address")]
    pub obligation_address: String,
    pub refreshed_stats: Stats,
    pub deposits: Positions,
    pub borrows: Positions,
}

/// The live API returns deposits/borrows as an object keyed by reserve (and
/// sometimes empty); older shapes and SDK dumps use an array. Accept both.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum Positions {
    List(Vec<Position>),
    Map(std::collections::HashMap<String, Position>),
}

impl Default for Positions {
    fn default() -> Self {
        Positions::List(Vec::new())
    }
}

impl Positions {
    fn into_vec(self) -> Vec<Position> {
        match self {
            Positions::List(v) => v,
            Positions::Map(m) => m.into_values().collect(),
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Stats {
    #[serde(alias = "userTotalDepositUsd", alias = "totalDepositUsd")]
    pub user_total_deposit: Num,
    #[serde(alias = "userTotalBorrowUsd", alias = "totalBorrowUsd")]
    pub user_total_borrow: Num,
    #[serde(alias = "ltv")]
    pub loan_to_value: Num,
    #[serde(alias = "liquidationLtv", alias = "unhealthyLoanToValue")]
    pub liquidation_ltv: Num,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct Position {
    #[serde(alias = "symbol", alias = "liquidityToken", alias = "tokenSymbol")]
    pub token: String,
    #[serde(alias = "mintAddress")]
    pub mint: String,
    #[serde(
        alias = "marketValueRefreshed",
        alias = "marketValueUsd",
        alias = "amountUsd",
        alias = "usdValue"
    )]
    pub market_value: Num,
}

fn positions(v: &[Position]) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = v
        .iter()
        .map(|p| {
            let name = if p.token.is_empty() {
                let m = &p.mint;
                if m.len() > 8 { format!("{}..", &m[..8]) } else { m.clone() }
            } else {
                p.token.clone()
            };
            (name, p.market_value.val())
        })
        .filter(|(_, usd)| *usd > 0.0)
        .collect();
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Parse the `/users/{wallet}/obligations` response body.
pub fn parse_obligations(body: &str) -> Result<Vec<Obligation>, String> {
    let raw: Vec<ApiObligation> =
        serde_json::from_str(body).map_err(|e| format!("unexpected API shape: {e}"))?;
    Ok(raw
        .into_iter()
        .map(|o| {
            let deposits = positions(&o.deposits.into_vec());
            let borrows = positions(&o.borrows.into_vec());
            // Prefer refreshed stats; fall back to summing positions.
            let dep_usd = {
                let s = o.refreshed_stats.user_total_deposit.val();
                if s > 0.0 { s } else { deposits.iter().map(|p| p.1).sum() }
            };
            let bor_usd = {
                let s = o.refreshed_stats.user_total_borrow.val();
                if s > 0.0 { s } else { borrows.iter().map(|p| p.1).sum() }
            };
            let mut ltv = o.refreshed_stats.loan_to_value.val();
            // Some responses express LTV in percent; normalize to fraction.
            if ltv > 1.5 {
                ltv /= 100.0;
            }
            let mut liq = o.refreshed_stats.liquidation_ltv.val();
            if liq > 1.5 {
                liq /= 100.0;
            }
            if ltv <= 0.0 && dep_usd > 0.0 {
                ltv = bor_usd / dep_usd;
            }
            Obligation {
                address: o.obligation_address,
                deposits_usd: dep_usd,
                borrows_usd: bor_usd,
                ltv,
                liq_ltv: liq,
                top_deposits: deposits,
                top_borrows: borrows,
            }
        })
        .collect())
}
