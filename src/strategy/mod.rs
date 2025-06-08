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
    Order {
        order: Order,
    },
    Cancel {
        order_id: String,
        reason: String,
    },
    None,
    Continue {
        ticker: Ticker,
        stop_propagation: bool,
        reason: String,
    },
}

pub trait Strategy {
    fn on_marketplace_event(
        &mut self,
        event: MarketPlaceEvent,
    ) -> impl std::future::Future<Output = Result<StrategyAction>>;
}
