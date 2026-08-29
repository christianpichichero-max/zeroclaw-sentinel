//! The point of these tests is adversarial: a repay builder is only useful if
//! it refuses to hand over anything that is not the repay you asked for.

use prepare_repay::verify::{render, verify, InstructionsResponse};

const WALLET: &str = "4yhXa4iERFGma3T1HMMjH8nJ8EdFqbahCV5Yomm8Z3do";
const RESERVE: &str = "D6q6wuQSrifJKZYpR1M8R4YawnLDtDsMmWM1NbBmgJ59";
const KLEND: &str = "KLend2g3cP87fffoy8q1mQqGKjrxjC8boSyAYavgmjD";

/// A live-shaped response: compute budget, three refresh_reserve, one
/// refresh_obligation, then repay_obligation_liquidity_v2 for 0.1 (100000
/// base units at 6 decimals). Byte pattern captured from the real API.
fn live_shaped(amount_b64: &str, owner: &str, reserve: &str) -> InstructionsResponse {
    let json = format!(
        r#"{{
        "instructions": [
          {{"programAddress":"ComputeBudget111111111111111111111111111111","data":"AkBCDwA=","accounts":[]}},
          {{"programAddress":"{KLEND}","data":"AtqK60/JGWY=","accounts":[]}},
          {{"programAddress":"{KLEND}","data":"IYST5JfASFk=","accounts":[]}},
          {{"programAddress":"{KLEND}","data":"{amount_b64}","accounts":[
             {{"address":"{owner}","role":"READONLY_SIGNER"}},
             {{"address":"ObLiGaTiOn1111111111111111111111111111111","role":"WRITABLE"}},
             {{"address":"7u3HeHxYDLhnCoErrtycNokbQYbWGzLs6JSDqGAv5PfF","role":"READONLY"}},
             {{"address":"{reserve}","role":"WRITABLE"}}
          ]}}
        ],
        "lutsByAddress": {{"FGMSBiyVE8TvZcdQnZETAAKw28tkQJ2ccZy6pyp95URb": []}}
      }}"#
    );
    serde_json::from_str(&json).expect("fixture parses")
}

/// repay_obligation_liquidity_v2 discriminator + 100000u64 LE.
const REPAY_100000: &str = "dK7VTLQ10pCghgEAAAAAAA==";

#[test]
fn accepts_a_genuine_repay_and_decodes_the_amount() {
    let r = live_shaped(REPAY_100000, WALLET, RESERVE);
    let v = verify(&r, WALLET, RESERVE, "0.1");
    assert!(v.ok, "should verify: {:?}", v.problem);
    assert_eq!(v.amount_base_units, 100_000);
    assert_eq!(v.implied_decimals, Some(6));
    assert!(v.caller_sets_lifetime, "no blockhash means the caller sets expiry");
    assert_eq!(v.lookup_tables.len(), 1);
}

#[test]
fn rejects_an_amount_that_does_not_match_the_request() {
    // Instruction repays 100000 base units; we asked for 5.0 tokens. At no
    // token scale is 5.0 equal to 100000, so this must fail closed.
    let r = live_shaped(REPAY_100000, WALLET, RESERVE);
    let v = verify(&r, WALLET, RESERVE, "5.0");
    assert!(!v.ok);
    assert!(v.problem.unwrap().contains("not 5"), "should name the mismatch");
}

#[test]
fn rejects_a_repay_owned_by_a_different_wallet() {
    let attacker = "Ev1LAttackerWa11et11111111111111111111111";
    let r = live_shaped(REPAY_100000, attacker, RESERVE);
    let v = verify(&r, WALLET, RESERVE, "0.1");
    assert!(!v.ok);
    assert!(v.problem.unwrap().contains("not the requested wallet"));
}

#[test]
fn rejects_a_different_reserve_than_requested() {
    let other = "OtherReserve11111111111111111111111111111";
    let r = live_shaped(REPAY_100000, WALLET, other);
    let v = verify(&r, WALLET, RESERVE, "0.1");
    assert!(!v.ok);
    assert!(v.problem.unwrap().contains("reserve"));
}

#[test]
fn rejects_a_foreign_program_smuggled_into_the_set() {
    // The attack this check exists for: the API returns a token transfer
    // alongside (or instead of) the repay.
    let json = format!(
        r#"{{"instructions":[
        {{"programAddress":"TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA","data":"AwEAAAA=","accounts":[]}},
        {{"programAddress":"{KLEND}","data":"{REPAY_100000}","accounts":[
          {{"address":"{WALLET}","role":"READONLY_SIGNER"}},
          {{"address":"{RESERVE}","role":"WRITABLE"}}]}}
      ],"lutsByAddress":{{}}}}"#
    );
    let r: InstructionsResponse = serde_json::from_str(&json).unwrap();
    let v = verify(&r, WALLET, RESERVE, "0.1");
    assert!(!v.ok);
    assert!(v.problem.unwrap().contains("unexpected program"));
}

#[test]
fn rejects_a_set_with_no_repay_instruction_at_all() {
    let json = format!(
        r#"{{"instructions":[
        {{"programAddress":"{KLEND}","data":"AtqK60/JGWY=","accounts":[]}}
      ],"lutsByAddress":{{}}}}"#
    );
    let r: InstructionsResponse = serde_json::from_str(&json).unwrap();
    let v = verify(&r, WALLET, RESERVE, "0.1");
    assert!(!v.ok);
    assert!(v.problem.unwrap().contains("no repay instruction"));
}

#[test]
fn rejects_an_empty_response() {
    let r: InstructionsResponse =
        serde_json::from_str(r#"{"instructions":[],"lutsByAddress":{}}"#).unwrap();
    let v = verify(&r, WALLET, RESERVE, "0.1");
    assert!(!v.ok);
}

#[test]
fn rejects_a_zero_amount_repay() {
    // discriminator + 0u64
    let zero = "dK7VTLQ10pAAAAAAAAAAAA==";
    let r = live_shaped(zero, WALLET, RESERVE);
    let v = verify(&r, WALLET, RESERVE, "0.1");
    assert!(!v.ok);
}

#[test]
fn render_is_compact_and_states_custody_plainly() {
    let r = live_shaped(REPAY_100000, WALLET, RESERVE);
    let v = verify(&r, WALLET, RESERVE, "0.1");
    let out = render(&v, WALLET, "USDC", RESERVE, "0.1", "https://api.example/ktx");
    assert!(out.contains("\"prepared\":true"));
    assert!(out.contains("UNSIGNED"), "custody must be stated: {out}");
    assert!(out.len() < 1200, "tool return too verbose: {} chars", out.len());
}

#[test]
fn a_failed_verification_tells_the_operator_not_to_sign() {
    let r = live_shaped(REPAY_100000, WALLET, RESERVE);
    let v = verify(&r, WALLET, RESERVE, "999.0");
    let out = render(&v, WALLET, "USDC", RESERVE, "999.0", "https://api.example/ktx");
    assert!(out.contains("DO NOT SIGN"), "must refuse loudly: {out}");
    assert!(out.contains("\"prepared\":false"));
}
