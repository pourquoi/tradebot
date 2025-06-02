use marketplace::MarketPlaceEvent;
use portfolio::PortfolioEvent;
use serde::{Deserialize, Serialize};
use strategy::StrategyEvent;

pub mod marketplace;
pub mod order;
pub mod portfolio;
pub mod server;
pub mod state;
pub mod strategy;
pub mod ticker;
pub mod tui;
pub mod utils;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum AppEvent {
    Portfolio(PortfolioEvent),
    Strategy(StrategyEvent),
    MarketPlace(MarketPlaceEvent),
}
