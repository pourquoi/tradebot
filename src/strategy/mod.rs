use crate::marketplace::MarketPlaceEvent;

pub mod scalping;

pub trait Strategy {
    async fn on_marketplace_event(&mut self, event: MarketPlaceEvent);
}
