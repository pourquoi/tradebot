use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{marketplace::MarketPlaceEvent, order::Order, ticker::Ticker};

pub mod scalping;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum StrategyEvent {
    Action(StrategyAction),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum StrategyAction {
    None,
    PlaceOrder {
        order: Order,
    },
    Ignore {
        ticker: Ticker,
        reason: String,
        details: Option<String>,
    },
    Break {
        ticker: Ticker,
        reason: String,
        details: Option<String>,
    },
    Cancel {
        order_id: String,
        reason: String,
        details: Option<String>,
    },
}

pub trait Strategy {
    fn on_marketplace_event(
        &mut self,
        event: MarketPlaceEvent,
    ) -> impl std::future::Future<Output = Result<Vec<StrategyAction>>>;
}
