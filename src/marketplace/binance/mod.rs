use std::collections::HashMap;
use std::env::var;
use std::sync::{Arc, LazyLock};

use crate::marketplace::binance::utils::ceil_to_step;
use crate::order::Order;
use crate::portfolio::Asset;
use crate::ticker::Ticker;
use crate::AppEvent;
use account_api::AccountOverview;
use anyhow::anyhow;
use anyhow::{Context, Result};
use reqwest::Client;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use settings_api::ExchangeInfo;
use settings_api::SymbolInfoFilter;
use tokio::sync::broadcast::Sender;
use tokio::sync::RwLock;
use tracing::error;

use super::MarketplaceDataStream;
use super::MarketplaceSettingsApi;
use super::MarketplaceTradeApi;
use super::{MarketplaceAccountApi, MarketplaceDataApi};

static ENDPOINT: LazyLock<String> = LazyLock::new(|| var("BINANCE_ENDPOINT").unwrap());
static PUBLIC_ENDPOINT: LazyLock<String> = LazyLock::new(|| {
    let default = var("BINANCE_ENDPOINT").unwrap();
    var("BINANCE_PUBLIC_ENDPOINT").unwrap_or(default)
});
static STREAM_ENDPOINT: LazyLock<String> =
    LazyLock::new(|| var("BINANCE_STREAM_ENDPOINT").unwrap());
static WS_ENDPOINT: LazyLock<String> = LazyLock::new(|| var("BINANCE_WS_ENDPOINT").unwrap());

pub mod account_api;
pub mod account_stream;
pub mod data_api;
pub mod data_stream;
pub mod settings_api;
pub mod trade_api;
mod utils;

#[derive(Default, Debug, Clone)]
pub struct Binance {
    client: Client,
    exchange_info: Arc<RwLock<Option<ExchangeInfo>>>,
    account_overview: Arc<RwLock<Option<AccountOverview>>>,
}

impl Binance {
    pub fn new() -> Self {
        let client = Client::builder().build().unwrap();

        Self {
            client,
            ..Default::default()
        }
    }

    pub async fn init(&mut self, tickers: &[Ticker]) -> Result<()> {
        let mut exchange_info = self.exchange_info.write().await;
        *exchange_info = {
            let res = self.get_exchange_info(tickers).await?;
            Some(res)
        };
        if self.get_account_overview(true).await.is_err() {
            error!("Could not get binance account overview. Using hardcoded fees.")
        }
        Ok(())
    }
}

impl crate::marketplace::Marketplace for Binance {}

impl MarketplaceDataStream for Binance {
    async fn start_data_stream(
        &mut self,
        tickers: &Vec<Ticker>,
        tx: Sender<AppEvent>,
    ) -> anyhow::Result<()> {
        self.connect_stream(tickers, tx).await;
        Ok(())
    }
}

impl MarketplaceSettingsApi for Binance {
    async fn get_fees(&self) -> Decimal {
        let account = self.account_overview.read().await;
        let account = account.as_ref();
        match account {
            Some(account) => account.commission_rates.taker,
            None => dec!(0.001),
        }
    }

    async fn adjust_order_price_and_amount(&self, order: &mut Order) -> Result<()> {
        let exchange_info = self.exchange_info.read().await;
        let exchange_info = exchange_info.as_ref().context("Empty exchange info")?;

        let info = exchange_info.symbols.iter().find(|info| {
            info.quote_asset == order.ticker.quote && info.base_asset == order.ticker.base
        });
        let info = info.context("Ticker info not found")?;

        let mut step_size = None;
        let mut min_qty = None;
        let mut max_qty = None;
        let mut min_notional = None;

        for filter in &info.filters {
            match filter {
                SymbolInfoFilter::LotSize {
                    min_qty: min,
                    max_qty: max,
                    step_size: step,
                } => {
                    step_size = Some(*step);
                    min_qty = Some(*min);
                    max_qty = Some(*max);
                }
                SymbolInfoFilter::Notional { min_notional: min } => {
                    min_notional = Some(*min);
                }
                _ => {}
            }
        }

        let step_size = step_size.context("No step_size found")?;
        let min_qty = min_qty.context("No min_qty found")?;
        let max_qty = max_qty.context("No max_qty found")?;
        let min_notional = min_notional.context("No min_notional found")?;

        let price = order.price;
        let mut amount = order.amount;

        amount = ceil_to_step(amount, step_size);

        if amount < min_qty {
            amount = min_qty;
        }
        if amount > max_qty {
            amount = max_qty;
        }

        let notional = amount * price;
        if notional < min_notional {
            amount = ceil_to_step(min_notional / price, step_size);
        }

        if amount < min_qty || amount > max_qty {
            anyhow::bail!("Adjusted amount not within allowed quantity range");
        }

        order.amount = amount;
        order.quote_amount = ceil_to_step(order.amount * order.price, dec!(0.01));
        Ok(())
    }
}

impl MarketplaceDataApi for Binance {
    async fn get_candles(
        &self,
        ticker: &Ticker,
        interval: &str,
        from: Option<u64>,
        to: Option<u64>,
    ) -> Result<Vec<super::MarketplaceCandle>> {
        let candles = self
            .get_candles(format!("{}", ticker).as_str(), interval, from, to)
            .await?;
        Ok(candles
            .into_iter()
            .map(|candle| candle.into_candle_event(ticker.clone()))
            .collect())
    }
}

impl MarketplaceAccountApi for Binance {
    async fn get_account_assets(&mut self) -> Result<HashMap<String, Asset>> {
        let account_overview = self.get_account_overview(false).await?;

        let assets = account_overview
            .balances
            .iter()
            .map(|balance| {
                (
                    balance.asset.clone(),
                    Asset {
                        symbol: balance.asset.clone(),
                        amount: balance.free,
                        locked: balance.locked,
                        value: None,
                    },
                )
            })
            .collect::<HashMap<String, Asset>>();

        Ok(assets)
    }
}

impl MarketplaceTradeApi for Binance {
    async fn get_orders(&self, tickers: &[Ticker]) -> Result<Vec<Order>> {
        let mut orders = Vec::new();
        for ticker in tickers {
            for order in self
                .get_open_orders(ticker)
                .await?
                .iter()
                .flat_map(|order| order.try_into().map_err(|err| {
                    error!("Could not convert order : {err}");
                    anyhow::anyhow!("Could not convert order : {err}")
                }))
            {
                orders.push(order);
            }
        }

        Ok(orders)
    }

    async fn place_order(&mut self, order: &Order) -> Result<Order> {
        let res = Binance::place_order(self, order).await?;
        Order::try_from(res).map_err(|err| anyhow!(err))
    }
}
