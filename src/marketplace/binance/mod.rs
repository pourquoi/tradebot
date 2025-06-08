use std::collections::HashMap;
use std::sync::Arc;

use crate::marketplace::binance::utils::ceil_to_step;
use crate::order::Order;
use crate::portfolio::Asset;
use crate::ticker::Ticker;
use crate::AppEvent;
use account_api::AccountOverview;
use anyhow::anyhow;
use anyhow::{Context, Result};
use exchange_info_api::ExchangeInfo;
use exchange_info_api::SymbolInfoFilter;
use reqwest::Client;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tokio::sync::broadcast::Sender;
use tokio::sync::RwLock;

use super::MarketPlaceSettings;
use super::MarketPlaceStream;
use super::MarketPlaceTrade;
use super::{MarketPlaceAccount, MarketPlaceData};

const ENDPOINT: &str = "https://api.binance.com";
const PUBLIC_MARKET_ENDPOINT: &str = "https://data-api.binance.vision";

pub mod account_api;
pub mod exchange_info_api;
pub mod market_data_api;
pub mod market_data_stream;
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

    pub async fn init(&mut self, tickers: &Vec<Ticker>) -> Result<()> {
        let mut exchange_info = self.exchange_info.write().await;
        *exchange_info = {
            let res = self.get_exchange_info(tickers).await?;
            Some(res)
        };
        let _ = self.get_account_overview(true).await?;
        Ok(())
    }
}

impl crate::marketplace::MarketPlace for Binance {}

impl MarketPlaceStream for Binance {
    async fn start(&mut self, tickers: &Vec<Ticker>, tx: Sender<AppEvent>) {
        self.listen_trade_stream(&tickers, tx).await;
    }
}

impl MarketPlaceSettings for Binance {
    async fn get_fees(&self, _order: &Order) -> Decimal {
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
        Ok(())
    }
}

impl MarketPlaceData for Binance {
    async fn get_candles(
        &self,
        ticker: &Ticker,
        interval: &str,
        from: Option<u64>,
        to: Option<u64>,
    ) -> Result<Vec<super::CandleEvent>> {
        let candles = self
            .get_candles(format!("{}", ticker).as_str(), interval, from, to)
            .await?;
        Ok(candles
            .into_iter()
            .map(|candle| candle.into_candle_event(ticker.clone()))
            .collect())
    }
}

impl MarketPlaceAccount for Binance {
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
                        value: None,
                    },
                )
            })
            .collect::<HashMap<String, Asset>>();

        Ok(assets)
    }
}

impl MarketPlaceTrade for Binance {
    async fn get_orders(&self, tickers: &Vec<Ticker>) -> Result<Vec<Order>> {
        let mut orders = Vec::new();
        for ticker in tickers {
            for order in self
                .get_open_orders(ticker)
                .await?
                .iter()
                .flat_map(|order| order.try_into())
                .into_iter()
            {
                orders.push(order);
            }
        }

        Ok(orders)
    }

    async fn place_order(&self, order: &Order) -> Result<Order> {
        let res = self.place_order(order).await?;
        Order::try_from(res).map_err(|err| anyhow!(err))
    }
}
