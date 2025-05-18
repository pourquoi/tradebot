use anyhow::Result;
use rust_decimal::Decimal;
use tokio::sync::broadcast::Sender;

pub mod binance;

#[derive(Clone, Debug)]
pub enum MarketPlaceEvent {
    TickerPrice(TickerPriceEvent),
}

#[derive(Clone, Debug)]
pub struct TickerPriceEvent {
    ticker: String,
    price: Decimal,
}

pub trait MarketPlace {
    async fn start(&self, tickers: Vec<String>, tx: Sender<MarketPlaceEvent>);
    async fn ping(&self) -> Result<()>;
}
