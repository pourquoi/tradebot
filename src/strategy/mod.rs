use crate::{order::Order, ticker::Ticker, AppEvent};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::Sender;

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
    fn start(
        &mut self,
        tx_app: Sender<AppEvent>,
    ) -> impl std::future::Future<Output = Result<Vec<StrategyAction>>>;
}
