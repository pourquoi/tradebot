use crate::marketplace::TickerPriceEvent;

pub mod scalping;

pub trait Strategy {
    fn on_ticker_price(&self, event: TickerPriceEvent);
}
