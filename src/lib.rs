use marketplace::MarketPlaceEvent;
use serde::{Deserialize, Serialize};
use state::StateEvent;
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
    State(StateEvent),
    Strategy(StrategyEvent),
    MarketPlace(MarketPlaceEvent),
}
