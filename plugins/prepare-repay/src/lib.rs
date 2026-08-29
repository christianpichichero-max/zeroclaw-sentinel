//! ZeroClaw tool plugin: prepare an UNSIGNED Kamino repay.
//!
//! Custody tier T1 (Build): produces an instruction set the owner signs in
//! their own wallet. Holds no keys, signs nothing, submits nothing. Every
//! instruction it hands back has been decoded and checked locally first.

pub mod verify;

#[cfg(target_family = "wasm")]
mod component {
    wit_bindgen::generate!({
        path: "wit/v0",
        world: "tool-plugin",
        features: ["plugins-wit-v0"],
    });

    use std::collections::HashMap;

    use crate::verify::{render, verify, InstructionsResponse};
    use exports::zeroclaw::plugin::plugin_info::Guest as PluginInfo;
    use exports::zeroclaw::plugin::tool::{Guest as Tool, ToolResult};
    use zeroclaw::plugin::logging::{
        log_record, LogLevel, PluginAction, PluginEvent, PluginOutcome,
    };

    const PRIMARY_MARKET: &str = "7u3HeHxYDLhnCoErrtycNokbQYbWGzLs6JSDqGAv5PfF";

    struct PrepareRepay;

    #[derive(serde::Deserialize)]
    struct ExecuteArgs {
        wallet: String,
        amount: String,
        /// Token symbol to repay, e.g. "USDC". Resolved to a reserve address.
        #[serde(default)]
        token: Option<String>,
        /// Explicit reserve address, when the caller already knows it.
        #[serde(default)]
        reserve: Option<String>,
        #[serde(default)]
        market: Option<String>,
        #[serde(rename = "__config", default)]
        config: HashMap<String, String>,
    }

    fn fail(msg: String) -> ToolResult {
        ToolResult { success: false, output: String::new(), error: Some(msg) }
    }

    fn log(outcome: PluginOutcome, msg: &str) {
        log_record(
            LogLevel::Info,
            &PluginEvent {
                function_name: "prepare_repay::tool::execute".into(),
                action: PluginAction::Complete,
                outcome: Some(outcome),
                duration_ms: None,
                attrs: None,
                message: msg.into(),
            },
        );
    }

    fn looks_like_pubkey(s: &str) -> bool {
        (32..=44).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric())
    }

    /// Resolve a token symbol to its reserve address for this market.
    fn resolve_reserve(api_base: &str, market: &str, token: &str) -> Option<(String, String)> {
        let url = format!("{api_base}/kamino-market/{market}/reserves/metrics");
        let resp = waki::Client::new()
            .get(&url)
            .connect_timeout(std::time::Duration::from_secs(8))
            .send()
            .ok()?;
        if resp.status_code() != 200 {
            return None;
        }
        let body = resp.body().ok()?;
        let rows: serde_json::Value = serde_json::from_slice(&body).ok()?;
        rows.as_array()?.iter().find_map(|r| {
            let sym = r.get("liquidityToken")?.as_str()?;
            if sym.eq_ignore_ascii_case(token) {
                Some((r.get("reserve")?.as_str()?.to_string(), sym.to_string()))
            } else {
                None
            }
        })
    }

    impl PluginInfo for PrepareRepay {
        fn plugin_name() -> String {
            "prepare-repay".to_string()
        }
        fn plugin_version() -> String {
            "0.1.0".to_string()
        }
    }

    impl Tool for PrepareRepay {
        fn name() -> String {
            "prepare_repay".to_string()
        }

        fn description() -> String {
            "Prepare an UNSIGNED Kamino loan repayment for a wallet to sign \
             itself. Returns a verified instruction set with no blockhash, so \
             it can be signed later without expiring. This tool never holds \
             keys, never signs, and never submits anything - the owner signs in \
             their own wallet. Use it after lending_health reports a position \
             that needs de-risking."
                .to_string()
        }

        fn parameters_schema() -> String {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "wallet": {
                        "type": "string",
                        "description": "Base58 Solana wallet address that owns the loan."
                    },
                    "amount": {
                        "type": "string",
                        "description": "Amount to repay in whole tokens, as a decimal string, e.g. \"250.5\". Not base units."
                    },
                    "token": {
                        "type": "string",
                        "description": "Symbol of the borrowed token to repay, e.g. \"USDC\". Either token or reserve is required."
                    },
                    "reserve": {
                        "type": "string",
                        "description": "Optional explicit Kamino reserve address, if known."
                    },
                    "market": {
                        "type": "string",
                        "description": "Optional Kamino lending market pubkey; defaults to the primary main market."
                    }
                },
                "required": ["wallet", "amount"]
            })
            .to_string()
        }

        fn execute(args: String) -> Result<ToolResult, String> {
            let a: ExecuteArgs = match serde_json::from_str(&args) {
                Ok(v) => v,
                Err(e) => return Ok(fail(format!("invalid arguments: {e}"))),
            };
            let cfg = &a.config;
            let api_base = cfg
                .get("api_base")
                .filter(|v| !v.is_empty())
                .cloned()
                .unwrap_or_else(|| "https://api.kamino.finance".to_string());
            let market = a
                .market
                .clone()
                .filter(|m| !m.is_empty())
                .or_else(|| cfg.get("market").filter(|v| !v.is_empty()).cloned())
                .unwrap_or_else(|| PRIMARY_MARKET.to_string());

            let wallet = a.wallet.trim().to_string();
            if !looks_like_pubkey(&wallet) {
                return Ok(fail("wallet does not look like a base58 Solana address".into()));
            }
            if a.amount.trim().parse::<f64>().map(|v| v <= 0.0).unwrap_or(true) {
                return Ok(fail("amount must be a positive decimal string".into()));
            }

            // Resolve which reserve we are repaying.
            let (reserve, token) = match (&a.reserve, &a.token) {
                (Some(r), _) if looks_like_pubkey(r) => {
                    (r.clone(), a.token.clone().unwrap_or_else(|| "?".into()))
                }
                (_, Some(t)) if !t.is_empty() => match resolve_reserve(&api_base, &market, t) {
                    Some(pair) => pair,
                    None => {
                        return Ok(fail(format!(
                            "no reserve found for token {t} in this market"
                        )))
                    }
                },
                _ => return Ok(fail("provide either token (e.g. USDC) or reserve".into())),
            };

            // Ask Kamino to build the instructions. This endpoint returns no
            // blockhash and no fee payer, which is what lets the human sign
            // later against a durable nonce instead of racing a ~90s expiry.
            let endpoint = format!("{api_base}/ktx/klend/repay-instructions");
            let payload = serde_json::json!({
                "wallet": wallet,
                "market": market,
                "reserve": reserve,
                "amount": a.amount.trim(),
            })
            .to_string();

            let resp = match waki::Client::new()
                .post(&endpoint)
                .header("Content-Type", "application/json")
                .body(payload.into_bytes())
                .connect_timeout(std::time::Duration::from_secs(12))
                .send()
            {
                Ok(r) => r,
                Err(e) => {
                    log(PluginOutcome::Failure, "kamino ktx request failed");
                    return Ok(fail(format!("Kamino transactions API unreachable: {e}")));
                }
            };
            let status = resp.status_code();
            let body = match resp.body() {
                Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                Err(e) => return Ok(fail(format!("failed reading API response: {e}"))),
            };
            if status != 200 {
                let head: String = body.chars().take(160).collect();
                return Ok(fail(format!("Kamino API returned {status}: {head}")));
            }

            let parsed: InstructionsResponse = match serde_json::from_str(&body) {
                Ok(p) => p,
                Err(e) => return Ok(fail(format!("unexpected instructions shape: {e}"))),
            };

            // Never hand over instructions we have not checked ourselves.
            let verdict = verify(&parsed, &wallet, &reserve, a.amount.trim());
            let out = render(&verdict, &wallet, &token, &reserve, a.amount.trim(), &endpoint);
            if verdict.ok {
                log(PluginOutcome::Success, "repay prepared and verified");
            } else {
                log(PluginOutcome::Failure, "prepared repay failed verification");
            }
            // A failed check is a result the model must reason about, not a
            // plugin fault: return it as a normal tool response.
            Ok(ToolResult { success: true, output: out, error: None })
        }
    }

    export!(PrepareRepay);
}
