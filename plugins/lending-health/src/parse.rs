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
    /// Raw on-chain account snapshot. Carries the per-reserve breakdown that
    /// the top-level maps omit, plus the slot of the last on-chain refresh.
    pub state: State,
}

/// On-chain obligation state. Values here are as of `last_update.slot`, which
/// can be far behind head: a position only refreshes when someone touches it.
/// Use it for composition (which tokens), never for current dollar values.
#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct State {
    pub last_update: LastUpdate,
    pub deposits: Vec<StatePos>,
    pub borrows: Vec<StatePos>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct LastUpdate {
    pub slot: Num,
    /// 0 = fresh, non-zero = the program considers this account stale.
    pub stale: Num,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct StatePos {
    #[serde(alias = "depositReserve", alias = "borrowReserve")]
    pub reserve: String,
    /// Market value as a scaled fraction (2^60). Stale — proportions only.
    pub market_value_sf: Num,
}

/// Kamino encodes fractional values as scaled fractions with a 2^60 factor.
const SCALED_FRACTION: f64 = 1_152_921_504_606_846_976.0; // 2^60

impl StatePos {
    fn usd(&self) -> f64 {
        self.market_value_sf.val() / SCALED_FRACTION
    }
    fn is_real(&self) -> bool {
        !self.reserve.is_empty() && !self.reserve.starts_with("111111") && self.usd() > 0.0
    }
}

/// Reserve-address -> token symbol, from the market's reserves/metrics feed.
pub type ReserveMap = std::collections::HashMap<String, String>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReserveRow {
    reserve: String,
    #[serde(default)]
    liquidity_token: String,
}

/// Parse `/kamino-market/{market}/reserves/metrics` into reserve -> symbol.
/// A failure here is non-fatal: the report degrades to reserve prefixes.
pub fn parse_reserves(body: &str) -> ReserveMap {
    serde_json::from_str::<Vec<ReserveRow>>(body)
        .map(|rows| {
            rows.into_iter()
                .filter(|r| !r.liquidity_token.is_empty())
                .map(|r| (r.reserve, r.liquidity_token))
                .collect()
        })
        .unwrap_or_default()
}

fn compose(positions: &[StatePos], reserves: &ReserveMap) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = positions
        .iter()
        .filter(|p| p.is_real())
        .map(|p| {
            let name = reserves.get(&p.reserve).cloned().unwrap_or_else(|| {
                let r = &p.reserve;
                if r.len() > 6 { format!("{}..", &r[..6]) } else { r.clone() }
            });
            (name, p.usd())
        })
        .collect();
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    out
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
pub fn parse_obligations(body: &str, reserves: &ReserveMap) -> Result<Vec<Obligation>, String> {
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
            let state_deposits = compose(&o.state.deposits, reserves);
            let state_borrows = compose(&o.state.borrows, reserves);
            Obligation {
                address: o.obligation_address,
                last_update_slot: o.state.last_update.slot.val() as u64,
                onchain_stale: o.state.last_update.stale.val() != 0.0,
                collateral_tokens: if deposits.is_empty() { state_deposits } else { deposits.clone() },
                debt_tokens: if borrows.is_empty() { state_borrows } else { borrows.clone() },
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
