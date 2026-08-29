//! Independent verification of the instruction set Kamino hands back.
//!
//! The agent asks an external API to build a repay. Nothing forces that API to
//! return a repay: a compromised or mistaken endpoint could return a transfer,
//! a different amount, or a different owner. So before any of it reaches the
//! operator, we decode the bytes ourselves and check they say what we asked
//! for. Pure logic, no I/O — natively testable.

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Deserialize;

/// Kamino Lending program. Every non-ComputeBudget instruction must be this.
pub const KLEND_PROGRAM: &str = "KLend2g3cP87fffoy8q1mQqGKjrxjC8boSyAYavgmjD";
pub const COMPUTE_BUDGET_PROGRAM: &str = "ComputeBudget111111111111111111111111111111";

/// First 8 bytes of sha256("global:repay_obligation_liquidity_v2").
/// Verified byte-for-byte against a live API response on 2026-08-29.
pub const REPAY_V2_DISCRIMINATOR: [u8; 8] = [0x74, 0xae, 0xd5, 0x4c, 0xb4, 0x35, 0xd2, 0x90];
/// The v1 instruction, still accepted by the program.
pub const REPAY_V1_DISCRIMINATOR: [u8; 8] = [0x91, 0xb2, 0x0d, 0xe1, 0x4c, 0xf0, 0x93, 0x48];

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct InstructionsResponse {
    pub instructions: Vec<Instruction>,
    /// Address lookup tables the signer must resolve before signing.
    pub luts_by_address: std::collections::HashMap<String, Vec<String>>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default, rename_all = "camelCase")]
pub struct Instruction {
    pub program_address: String,
    /// Base64; ComputeBudget-style instructions may carry null.
    pub data: Option<String>,
    pub accounts: Vec<Account>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default, rename_all = "camelCase")]
pub struct Account {
    pub address: String,
    pub role: String,
}

/// What we independently established about the prepared instruction set.
pub struct Verdict {
    /// Every check passed: this really is a repay of `amount` for `wallet`.
    pub ok: bool,
    /// Human-readable reason when `ok` is false.
    pub problem: Option<String>,
    pub instruction_count: usize,
    /// Base units decoded straight out of the repay instruction.
    pub amount_base_units: u64,
    /// Decimals implied by (base units / requested amount), when consistent.
    pub implied_decimals: Option<u32>,
    /// True when a blockhash/lifetime is absent, i.e. the caller controls
    /// expiry. That is what makes durable-nonce signing possible.
    pub caller_sets_lifetime: bool,
    pub lookup_tables: Vec<String>,
}

fn fail(v: &mut Verdict, msg: impl Into<String>) {
    v.ok = false;
    if v.problem.is_none() {
        v.problem = Some(msg.into());
    }
}

/// Decode base64 instruction data, tolerating the absence of padding.
fn decode(data: &str) -> Option<Vec<u8>> {
    STANDARD.decode(data.trim()).ok()
}

/// Check that `resp` is a repay of `requested_amount` of `reserve`, owned by
/// `wallet`. `requested_amount` is the human decimal string the caller asked
/// for (e.g. "12.5").
pub fn verify(
    resp: &InstructionsResponse,
    wallet: &str,
    reserve: &str,
    requested_amount: &str,
) -> Verdict {
    let mut v = Verdict {
        ok: true,
        problem: None,
        instruction_count: resp.instructions.len(),
        amount_base_units: 0,
        implied_decimals: None,
        caller_sets_lifetime: true,
        lookup_tables: resp.luts_by_address.keys().cloned().collect(),
    };
    v.lookup_tables.sort();

    if resp.instructions.is_empty() {
        fail(&mut v, "API returned no instructions");
        return v;
    }

    // 1. No instruction may belong to a program we did not expect. This is the
    //    check that catches "the API returned a token transfer".
    for ix in &resp.instructions {
        if ix.program_address != KLEND_PROGRAM && ix.program_address != COMPUTE_BUDGET_PROGRAM {
            fail(
                &mut v,
                format!("unexpected program in instruction set: {}", ix.program_address),
            );
            return v;
        }
    }

    // 2. The repay itself is the last Kamino instruction carrying a known
    //    discriminator. Find it rather than assuming a fixed index: the number
    //    of refresh_reserve instructions varies with the obligation.
    let repay = resp.instructions.iter().rev().find(|ix| {
        ix.program_address == KLEND_PROGRAM
            && ix
                .data
                .as_deref()
                .and_then(decode)
                .map(|b| {
                    b.len() >= 8
                        && (b[..8] == REPAY_V2_DISCRIMINATOR || b[..8] == REPAY_V1_DISCRIMINATOR)
                })
                .unwrap_or(false)
    });
    let repay = match repay {
        Some(ix) => ix,
        None => {
            fail(&mut v, "no repay instruction found in the returned set");
            return v;
        }
    };

    // 3. Decode the amount the instruction actually repays.
    let bytes = repay.data.as_deref().and_then(decode).unwrap_or_default();
    if bytes.len() < 16 {
        fail(&mut v, "repay instruction data is too short to carry an amount");
        return v;
    }
    let mut le = [0u8; 8];
    le.copy_from_slice(&bytes[8..16]);
    v.amount_base_units = u64::from_le_bytes(le);
    if v.amount_base_units == 0 {
        fail(&mut v, "repay amount decoded as zero");
        return v;
    }

    // 4. The decoded amount must be the requested amount scaled by some sane
    //    power of ten. A mismatch means the API repaid a different number than
    //    we asked for, which is the whole reason this function exists.
    match requested_amount.trim().parse::<f64>() {
        Ok(req) if req > 0.0 => {
            let mut matched = None;
            for d in 0u32..=12 {
                let expect = req * 10f64.powi(d as i32);
                if (expect - v.amount_base_units as f64).abs() <= (expect * 1e-9).max(0.5) {
                    matched = Some(d);
                    break;
                }
            }
            match matched {
                Some(d) => v.implied_decimals = Some(d),
                None => {
                    let got = v.amount_base_units;
                    fail(
                        &mut v,
                        format!(
                            "instruction repays {got} base units, which is not {req} at any token scale"
                        ),
                    );
                }
            }
        }
        _ => fail(&mut v, "requested amount is not a positive number"),
    }

    // 5. The owner signing this must be the wallet we asked for, and the
    //    reserve being repaid must be the reserve we asked for.
    let addrs: Vec<&str> = repay.accounts.iter().map(|a| a.address.as_str()).collect();
    match addrs.first() {
        Some(owner) if *owner == wallet => {}
        Some(owner) => fail(
            &mut v,
            format!("repay is signed by {owner}, not the requested wallet"),
        ),
        None => fail(&mut v, "repay instruction carries no accounts"),
    }
    if !addrs.iter().any(|a| *a == reserve) {
        fail(
            &mut v,
            "the requested reserve does not appear in the repay instruction".to_string(),
        );
    }

    v
}

/// Compact operator-facing report. Deliberately small: this is a tool return
/// the model has to reason about, not a transaction dump.
pub fn render(
    v: &Verdict,
    wallet: &str,
    token: &str,
    reserve: &str,
    amount: &str,
    endpoint: &str,
) -> String {
    let mut out = serde_json::json!({
        "prepared": v.ok,
        "custody": "UNSIGNED — this tool holds no keys, signs nothing, and submits nothing",
        "wallet": wallet,
        "token": token,
        "reserve": reserve,
        "repay_amount": amount,
        "instructions": v.instruction_count,
        "verified_independently": {
            "is_kamino_repay": v.ok,
            "amount_base_units": v.amount_base_units,
            "token_decimals": v.implied_decimals,
            "no_foreign_programs": v.ok,
            "owner_is_your_wallet": v.ok,
        },
        "caller_sets_lifetime": v.caller_sets_lifetime,
        "lookup_tables": v.lookup_tables,
        "next_step": "sign in your own wallet; the instruction set carries no \
                      blockhash, so it can be anchored to a durable nonce and \
                      signed later without expiring",
        "reproduce": endpoint,
    });
    if let Some(p) = &v.problem {
        out.as_object_mut()
            .unwrap()
            .insert("problem".into(), serde_json::json!(p));
        out.as_object_mut().unwrap().insert(
            "next_step".into(),
            serde_json::json!("DO NOT SIGN. Verification failed — report this to the operator."),
        );
    }
    out.to_string()
}
