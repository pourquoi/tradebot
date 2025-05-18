use tracing::info;

use crate::marketplace::TickerPriceEvent;
use crate::strategy::Strategy;

#[derive(Clone, Debug)]
pub struct ScalpingStrategy {}

impl Strategy for ScalpingStrategy {
    fn on_ticker_price(&self, event: TickerPriceEvent) {
        info!("{:?}", event);
    }
}
