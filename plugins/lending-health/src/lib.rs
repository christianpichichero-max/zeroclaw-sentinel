//! ZeroClaw tool plugin: Kamino lending health for a Solana wallet.
//! Read-only (T0 custody): fetches public API state, computes health/LTV/
//! distance-to-liquidation, returns a compact JSON report. Never touches keys.

pub mod health;
pub mod parse;

#[cfg(target_family = "wasm")]
mod component {
    wit_bindgen::generate!({
        path: "wit/v0",
        world: "tool-plugin",
        features: ["plugins-wit-v0"],
    });

    use std::collections::HashMap;

    use crate::health::{render, Config};
    use crate::parse::parse_obligations;
    use exports::zeroclaw::plugin::plugin_info::Guest as PluginInfo;
    use exports::zeroclaw::plugin::tool::{Guest as Tool, ToolResult};
    use zeroclaw::plugin::logging::{
        log_record, LogLevel, PluginAction, PluginEvent, PluginOutcome,
    };

    struct LendingHealth;

    #[derive(serde::Deserialize)]
    struct ExecuteArgs {
        wallet: String,
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
                function_name: "lending_health::tool::execute".into(),
                action: PluginAction::Complete,
                outcome: Some(outcome),
                duration_ms: None,
                attrs: None,
                message: msg.into(),
            },
        );
    }

    impl PluginInfo for LendingHealth {
        fn plugin_name() -> String {
            "lending-health".to_string()
        }
        fn plugin_version() -> String {
            "0.1.0".to_string()
        }
    }

    impl Tool for LendingHealth {
        fn name() -> String {
            "lending_health".to_string()
        }

        fn description() -> String {
            "Check the Kamino lending health of a Solana wallet: deposits, \
             borrows, current vs liquidation LTV, how far collateral can drop \
             before liquidation, and the exact repay/add-collateral amounts to \
             reach a safe LTV. Read-only public data; makes an outbound HTTPS \
             request and may surface an operator approval prompt."
                .to_string()
        }

        fn parameters_schema() -> String {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "wallet": {
                        "type": "string",
                        "description": "Base58 Solana wallet address to check."
                    },
                    "market": {
                        "type": "string",
                        "description": "Optional Kamino lending market pubkey; defaults to the primary main market."
                    }
                },
                "required": ["wallet"]
            })
            .to_string()
        }

        fn execute(args: String) -> Result<ToolResult, String> {
            let parsed: ExecuteArgs = match serde_json::from_str(&args) {
                Ok(a) => a,
                Err(e) => return Ok(fail(format!("invalid arguments: {e}"))),
            };
            let wallet = parsed.wallet.trim().to_string();
            if wallet.len() < 32
                || wallet.len() > 44
                || !wallet.chars().all(|c| c.is_ascii_alphanumeric())
            {
                return Ok(fail("wallet does not look like a base58 Solana address".into()));
            }

            let cfg = Config::from_section(&parsed.config);
            let market = parsed
                .market
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| cfg.market.clone());
            let url = format!(
                "{}/kamino-market/{}/users/{}/obligations",
                cfg.api_base, market, wallet
            );

            let resp = match waki::Client::new()
                .get(&url)
                .connect_timeout(std::time::Duration::from_secs(10))
                .send()
            {
                Ok(r) => r,
                Err(e) => {
                    log(PluginOutcome::Failure, "kamino api request failed");
                    return Ok(fail(format!("Kamino API unreachable: {e}")));
                }
            };
            let status = resp.status_code();
            let body = match resp.body() {
                Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                Err(e) => return Ok(fail(format!("failed reading API response: {e}"))),
            };
            if status != 200 {
                let head: String = body.chars().take(120).collect();
                return Ok(fail(format!("Kamino API returned {status}: {head}")));
            }

            let obligations = match parse_obligations(&body) {
                Ok(o) => o,
                Err(e) => {
                    log(PluginOutcome::Failure, "unexpected api shape");
                    return Ok(fail(e));
                }
            };

            let output = render(&wallet, &market, &obligations, &cfg);
            log(PluginOutcome::Success, "health computed");
            Ok(ToolResult { success: true, output, error: None })
        }
    }

    export!(LendingHealth);
}
