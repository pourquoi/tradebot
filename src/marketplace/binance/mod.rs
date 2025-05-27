use anyhow::{Context, Result};
use info::ExchangeInfo;
use info::SymbolInfoFilter;
use reqwest::Client;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tokio::sync::broadcast::Sender;

use crate::order::Order;
use crate::ticker::Ticker;

mod candles;
mod info;
mod stream;

const ENDPOINT: &str = "https://api.binance.com";
const MARKET_ENDPOINT: &str = "https://data-api.binance.vision";

#[derive(Default, Debug, Clone)]
pub struct Binance {
    client: Client,
    exchange_info: Option<ExchangeInfo>,
}

impl Binance {
    pub fn new() -> Self {
        let client = Client::builder().build().unwrap();
        Self {
            client,
            ..Default::default()
        }
    }
}

impl crate::marketplace::MarketPlace for Binance {
    fn get_fees(order: &Order) -> Decimal {
        dec!(0.001)
    }

    async fn init(&mut self, tickers: Vec<Ticker>) -> Result<()> {
        let exchange_info = self.get_exchange_info(&tickers).await?;
        self.exchange_info = Some(exchange_info);
        Ok(())
    }

    fn adjust_order_price_and_amount(&self, order: &mut Order) -> Result<()> {
        let exchange_info = self
            .exchange_info
            .as_ref()
            .context("No exchange info yet")?;

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

    async fn start(
        &mut self,
        tickers: Vec<Ticker>,
        tx: Sender<crate::marketplace::MarketPlaceEvent>,
    ) {
        self.exchange_info = Some(self.get_exchange_info(&tickers).await.unwrap());

        self.listen_trade_stream(tickers, tx).await;
    }

    async fn ping(&self) -> Result<()> {
        let _ = self
            .client
            .get(format!("{ENDPOINT}/api/v3/ping"))
            .send()
            .await?;
        Ok(())
    }
}

fn ceil_to_step(value: Decimal, step: Decimal) -> Decimal {
    ((value / step).ceil()) * step
}
