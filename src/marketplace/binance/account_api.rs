use std::env;

use anyhow::Result;
use chrono::prelude::*;
use hex::encode;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use serde::Deserialize;
use sha2::Sha256;
use tracing::debug;

use crate::marketplace::binance::Binance;

use super::ENDPOINT;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AccountOverview {
    pub uid: u64,
    pub balances: Vec<AccountBalance>,
    pub commission_rates: AccountCommissions,
}

#[derive(Deserialize, Debug, Clone)]
pub struct AccountBalance {
    pub asset: String,
    #[serde(with = "rust_decimal::serde::str")]
    pub free: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub locked: Decimal,
}

#[derive(Deserialize, Debug, Clone)]
pub struct AccountCommissions {
    #[serde(with = "rust_decimal::serde::str")]
    pub maker: Decimal,
    #[serde(with = "rust_decimal::serde::str")]
    pub taker: Decimal,
}

impl Binance {
    pub async fn get_account_overview(&self) -> Result<AccountOverview> {
        let api_key = env::var("BINANCE_API_KEY")?;
        let api_secret = env::var("BINANCE_API_SECRET")?;

        let timestamp = Utc::now().timestamp_millis();
        let params = format!("timestamp={}&omitZeroBalances=true", timestamp);

        let mut mac: Hmac<Sha256> = Hmac::new_from_slice(api_secret.as_bytes())?;
        mac.update(params.as_bytes());
        let signature = mac.finalize();
        let signature = encode(signature.into_bytes());

        let params = [
            ("timestamp", timestamp.to_string()),
            ("omitZeroBalances", "true".to_string()),
            ("signature", signature),
        ];

        let url = reqwest::Url::parse_with_params(
            format!("{}/api/v3/account", ENDPOINT).as_str(),
            params,
        )?;
        let res = self
            .client
            .get(url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await?;

        let body = res.text().await?;

        debug!("Account info : {}", body);

        let overview = serde_json::de::from_str(body.as_str())?;

        Ok(overview)
    }
}
