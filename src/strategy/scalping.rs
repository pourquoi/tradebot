use super::StrategyAction;
use crate::marketplace::{
    Marketplace, MarketplaceBook, MarketplaceCandle, MarketplaceDataApi, MarketplaceEvent,
    MarketplaceSettingsApi, MarketplaceTrade,
};
use crate::order::{Order, OrderSide, OrderStatus};
use crate::state::{OrderListFilters, OrderListSort, OrderListSortBy, State};
use crate::strategy::{Strategy, StrategyEvent};
use crate::ticker::Ticker;
use crate::utils::{atr, find_price_clusters, sma, wsma};
use crate::AppEvent;
use anyhow::{Context, Result};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::Sender;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

#[derive(Clone, Debug)]
pub struct ScalpingStrategy<M> {
    marketplace: M,
    ticker: Ticker,
    state: Arc<RwLock<State>>,
    trade_event_history: Arc<RwLock<VecDeque<MarketplaceTrade>>>,
    candle_event_history: Arc<RwLock<VecDeque<MarketplaceCandle>>>,
    price_stats: Arc<RwLock<PriceStats>>,
    initialized: bool,
    params: ScalpingParams,
}

#[derive(Clone, Debug)]
pub struct ScalpingParams {
    pub target_profit: Decimal,
    pub quote_amount: Decimal,
    pub entry_delay: Duration,
    pub reentry_delay: Duration,
    pub session_count: u8,
    pub session_profit_lifetime: Duration,
}

#[derive(Clone, Debug, Default)]
struct PriceStats {
    pub short_trend: Option<PriceTrend>,
    pub long_trend: Option<PriceTrend>,
    pub short_support: Option<Decimal>,
    pub long_support: Option<Decimal>,
    pub short_resistance: Option<Decimal>,
    pub long_resistance: Option<Decimal>,
}

#[derive(Clone, Debug)]
enum PriceTrend {
    Up,
    Down,
    Bull,
    Crash,
}

impl<M: Marketplace + MarketplaceSettingsApi + MarketplaceDataApi> ScalpingStrategy<M> {
    pub fn new(
        state: Arc<RwLock<State>>,
        marketplace: M,
        ticker: Ticker,
        params: ScalpingParams,
    ) -> Self {
        Self {
            state,
            ticker,
            trade_event_history: Arc::from(RwLock::from(VecDeque::new())),
            candle_event_history: Arc::from(RwLock::from(VecDeque::new())),
            price_stats: Arc::from(RwLock::from(PriceStats::default())),
            params,
            marketplace,
            initialized: false,
        }
    }

    pub async fn init(&mut self, start_time: Option<u64>) -> Result<()> {
        if self.initialized {
            return Ok(());
        }

        let mut candles = self
            .marketplace
            .get_candles(&self.ticker, "1m", None, start_time)
            .await?;
        let mut history = self.candle_event_history.write().await;
        candles.reverse();
        info!(
            "Loaded {} candles for {}. Start={:?} End={:?}",
            candles.len(),
            self.ticker,
            candles.first().map(|candle| candle.start_time),
            candles.last().map(|candle| candle.start_time),
        );
        *history = VecDeque::from(candles);

        self.initialized = true;

        Ok(())
    }

    async fn add_trade_event_history(&mut self, event: MarketplaceTrade) {
        let mut history = self.trade_event_history.write().await;

        history.push_front(event);
        if history.len() > 500 {
            history.pop_back();
        }
    }

    fn get_sma(&self, history: &[MarketplaceCandle], n: usize) -> Option<Decimal> {
        let price_history: Vec<Decimal> = history.iter().map(|event| event.close_price).collect();

        sma(&price_history, n)
    }

    async fn add_candle_event_history(&mut self, event: MarketplaceCandle) {
        let mut update_stats = false;
        {
            let mut history = self.candle_event_history.write().await;

            if let Some(last) = history.pop_front() {
                if last.start_time != event.start_time {
                    history.push_front(last);
                    update_stats = true;
                }
            }
            history.push_front(event.clone());

            if history.len() >= 120 {
                history.pop_back();
            }
        }

        if update_stats {
            self.update_stats(event.close_price, event.close_price)
                .await;
        }
    }

    fn get_atr(&self, history: &[MarketplaceCandle], n: usize) -> Option<Decimal> {
        let price_history: Vec<(Decimal, Decimal, Decimal)> = history
            .windows(2)
            .map(|candles| {
                (
                    candles[0].high_price,
                    candles[0].low_price,
                    candles[1].close_price,
                )
            })
            .collect();

        atr(&price_history, n)
    }

    fn get_wsma(&self, history: &[MarketplaceCandle], n: usize) -> Option<Decimal> {
        let price_history: Vec<Decimal> = history.iter().map(|event| event.close_price).collect();
        wsma(&price_history, n)
    }

    fn get_support(
        &self,
        history: &[MarketplaceCandle],
        n: usize,
        tolerance: Decimal,
        price: Decimal,
    ) -> Option<Decimal> {
        let prices: Vec<Decimal> = history
            .iter()
            .map(|event| event.low_price)
            .take(n)
            .collect();

        let supports =
            find_price_clusters(&prices, tolerance, crate::utils::PriceClusterSide::Support);
        supports
            .iter()
            .copied()
            .filter(|level| *level < price)
            .max()
    }

    fn get_resistance(
        &self,
        history: &[MarketplaceCandle],
        n: usize,
        tolerance: Decimal,
        price: Decimal,
    ) -> Option<Decimal> {
        let prices: Vec<Decimal> = history
            .iter()
            .map(|event| event.low_price)
            .take(n)
            .collect();

        let supports = find_price_clusters(
            &prices,
            tolerance,
            crate::utils::PriceClusterSide::Resistance,
        );
        supports
            .iter()
            .copied()
            .filter(|level| *level > price)
            .min()
    }

    async fn update_stats(&self, buy_price: Decimal, sell_price: Decimal) -> Option<PriceStats> {
        let history = self.candle_event_history.read().await;
        let history: Vec<MarketplaceCandle> = history.iter().cloned().collect();

        let wsma_120 = self.get_wsma(&history, 120)?;
        let wsma_14 = self.get_wsma(&history, 14)?;
        let wsma_5 = self.get_wsma(&history, 5)?;

        let sma_120 = self.get_sma(&history, 120)?;
        let sma_14 = self.get_sma(&history, 14)?;
        let sma_5 = self.get_sma(&history, 5)?;

        let atr_120 = self.get_atr(&history, 120)?;
        let atr_14 = self.get_atr(&history, 14)?;
        let atr_5 = self.get_atr(&history, 5)?;

        let support_120 = self.get_support(&history, 120, atr_120 * dec!(0.5), buy_price);
        let support_14 = self.get_support(&history, 14, atr_14 * dec!(0.5), buy_price);

        let resistance_120 = self.get_resistance(&history, 120, atr_120 * dec!(0.5), buy_price);
        let resistance_14 = self.get_resistance(&history, 14, atr_14 * dec!(0.5), buy_price);

        let mut short_trend = None;
        if wsma_5 > wsma_14 {
            if wsma_14 > dec!(0) && (wsma_5 - wsma_14) / wsma_14 > dec!(0.01) {
                short_trend = Some(PriceTrend::Bull)
            } else {
                short_trend = Some(PriceTrend::Up)
            }
        } else if wsma_5 < wsma_14 {
            if wsma_14 > dec!(0) && (wsma_14 - wsma_5) / wsma_14 > dec!(0.01) {
                short_trend = Some(PriceTrend::Crash)
            } else {
                short_trend = Some(PriceTrend::Down)
            }
        }

        let mut long_trend = None;
        if wsma_14 > wsma_120 {
            if wsma_120 > dec!(0) && (wsma_14 - wsma_120) / wsma_120 > dec!(0.01) {
                long_trend = Some(PriceTrend::Bull)
            } else {
                long_trend = Some(PriceTrend::Up)
            }
        } else if wsma_14 < wsma_120 {
            if wsma_120 > dec!(0) && (wsma_120 - wsma_14) / wsma_120 > dec!(0.01) {
                long_trend = Some(PriceTrend::Crash)
            } else {
                long_trend = Some(PriceTrend::Down)
            }
        }

        Some(PriceStats {
            short_trend,
            long_trend,
            short_support: support_14,
            long_support: support_120,
            short_resistance: resistance_14,
            long_resistance: resistance_120,
        })
    }

    async fn process_sell(
        &self,
        current_sell_price: Decimal,
        current_time: u64,
    ) -> Vec<StrategyAction> {
        let state = self.state.read().await;

        let last_buy_orders = state.find_by(
            OrderListFilters {
                ticker: Some(self.ticker.clone()),
                side: Some(OrderSide::Buy),
                status: vec![OrderStatus::Executed],
                has_child: Some(false),
                ..Default::default()
            },
            OrderListSort {
                by: OrderListSortBy::Date,
                asc: false,
            },
        );

        let mut actions: Vec<StrategyAction> = Vec::new();

        for buy_order in last_buy_orders.iter() {
            let fees = self.marketplace.get_fees().await;
            let amount = buy_order.filled_amount * (dec!(1) - fees);
            if !state.portfolio.check_funds(&self.ticker.base, amount) {
                actions.push(StrategyAction::Ignore {
                    ticker: self.ticker.clone(),
                    reason: "Sell no funds".to_string(),
                    details: Some(format!(
                        "{} / {}",
                        state
                            .portfolio
                            .assets
                            .get(&self.ticker.base)
                            .map(|asset| asset.amount)
                            .unwrap_or(dec!(0)),
                        amount,
                    )),
                });
                continue;
            }
            let price = current_sell_price;
            let order = Order::new_sell(
                self.ticker.clone(),
                amount,
                price,
                current_time,
                Some(buy_order),
            );
            let receive = amount * price * (dec!(1) - fees);
            let take_profit = receive - buy_order.cumulative_quote_amount;

            if take_profit < self.params.target_profit {
                //if take_profit < self.params.target_profit * -dec!(0.5) {
                //    actions.push(StrategyAction::PlaceOrder { order });
                //    break;
                //}
                actions.push(StrategyAction::Ignore {
                    ticker: self.ticker.clone(),
                    reason: "No profit".to_string(),
                    details: Some(format!("{}", take_profit)),
                });
                continue;
            }

            let stats = self.price_stats.read().await;

            let mut ignores: Vec<StrategyAction> = vec![];
            if matches!(stats.short_trend, Some(PriceTrend::Bull)) {
                ignores.push(StrategyAction::Ignore {
                    ticker: self.ticker.clone(),
                    reason: "Hold bull".to_string(),
                    details: None,
                });
            }

            if ignores.is_empty() {
                actions.push(StrategyAction::PlaceOrder { order });
                break;
            }

            actions.append(&mut ignores);
        }

        actions
    }

    async fn process_reentry(
        &self,
        current_buy_price: Decimal,
        current_time: u64,
    ) -> Vec<StrategyAction> {
        if current_buy_price <= dec!(0) {
            error!("Invalid current buy price for {}", self.ticker);
            return vec![];
        }

        let state = self.state.read().await;

        let last_sell_orders = state.find_by(
            OrderListFilters {
                ticker: Some(self.ticker.clone()),
                side: Some(OrderSide::Sell),
                has_child: Some(false),
                status: vec![
                    OrderStatus::Executed,
                    OrderStatus::Active,
                    OrderStatus::Sent,
                    OrderStatus::Draft,
                ],
                ..Default::default()
            },
            OrderListSort {
                by: OrderListSortBy::Date,
                asc: false,
            },
        );

        let mut actions: Vec<StrategyAction> = Vec::new();
        for sell_order in last_sell_orders.iter() {
            let amount = self.params.quote_amount / current_buy_price;
            let price = current_buy_price;

            if self.params.reentry_delay
                > Duration::from_millis(current_time.saturating_sub(sell_order.creation_time))
            {
                actions.push(StrategyAction::Ignore {
                    ticker: self.ticker.clone(),
                    reason: "Reentry delay".to_string(),
                    details: None,
                });
                continue;
            }

            if !state
                .portfolio
                .check_funds(&self.ticker.quote, self.params.quote_amount)
            {
                actions.push(StrategyAction::Ignore {
                    ticker: self.ticker.clone(),
                    reason: "Reentry no funds".to_string(),
                    details: Some(format!(
                        "{} / {}",
                        state
                            .portfolio
                            .assets
                            .get(&self.ticker.quote)
                            .map(|asset| asset.amount)
                            .unwrap_or(dec!(0)),
                        price,
                    )),
                });
                continue;
            }

            if let Some(session_id) = &sell_order.session_id {
                let session_profit = state.get_session_profit(session_id);
                if session_profit >= dec!(0)
                    && Duration::from_millis(
                        current_time
                            .saturating_sub(state.get_session_start(session_id).unwrap_or(0)),
                    ) > self.params.session_profit_lifetime
                {
                    actions.push(StrategyAction::Ignore {
                        ticker: self.ticker.clone(),
                        reason: "Terminating session".to_string(),
                        details: Some(format!("{}", session_profit)),
                    });
                    continue;
                }
            }

            let stats = self.price_stats.read().await;
            let mut ignores: Vec<StrategyAction> = vec![];

            if stats
                .short_resistance
                .is_some_and(|resistance| resistance > current_buy_price)
            {
                ignores.push(StrategyAction::Ignore {
                    ticker: self.ticker.clone(),
                    reason: "Reentry long resistance < price".to_string(),
                    details: None,
                });
            }
            if stats
                .long_resistance
                .is_some_and(|resistance| resistance > current_buy_price)
            {
                ignores.push(StrategyAction::Ignore {
                    ticker: self.ticker.clone(),
                    reason: "Reentry long resistance < price".to_string(),
                    details: None,
                });
            }

            if ignores.is_empty() {
                let mut order = Order::new_buy(
                    self.ticker.clone(),
                    amount,
                    price,
                    amount * price,
                    current_time,
                    Some(sell_order),
                );
                if self
                    .marketplace
                    .adjust_order_price_and_amount(&mut order)
                    .await
                    .is_ok()
                {
                    actions.push(StrategyAction::PlaceOrder { order });
                    break;
                }
            }

            actions.append(&mut ignores);
        }

        actions
    }

    async fn process_entry(
        &self,
        current_buy_price: Decimal,
        current_time: u64,
    ) -> Vec<StrategyAction> {
        if current_buy_price <= dec!(0) {
            error!("Invalid current buy price for {}", self.ticker);
            return vec![];
        }

        let state = self.state.read().await;

        let pending_buy_orders = state
            .orders
            .iter()
            .filter(|order| {
                order.ticker == self.ticker
                    && order.side == OrderSide::Buy
                    && matches!(order.status, OrderStatus::Sent | OrderStatus::Draft)
            })
            .count();

        if pending_buy_orders > 0 {
            debug!("Pending buy orders: skipping entry");
            return vec![];
        }

        if let Some(last_order_time) = state.get_last_executed_order_time(OrderListFilters {
            ticker: Some(self.ticker.clone()),
            ..Default::default()
        }) {
            if self.params.entry_delay
                > Duration::from_millis(current_time.saturating_sub(last_order_time))
            {
                debug!("Last order too recent: skipping entry");
                return vec![];
            }
        }

        let amount = self.params.quote_amount / current_buy_price;
        let price = current_buy_price;

        if !state
            .portfolio
            .check_funds(&self.ticker.quote, self.params.quote_amount)
        {
            return vec![StrategyAction::Ignore {
                ticker: self.ticker.clone(),
                reason: "Entry no funds".to_string(),
                details: Some(format!(
                    "{} / {}",
                    state
                        .portfolio
                        .assets
                        .get(&self.ticker.quote)
                        .map(|asset| asset.amount)
                        .unwrap_or(dec!(0)),
                    self.params.quote_amount,
                )),
            }];
        }

        if state.get_active_sessions(&self.ticker, current_time, &Duration::from_secs(3600))
            >= self.params.session_count.into()
        {
            return vec![StrategyAction::Ignore {
                ticker: self.ticker.clone(),
                reason: "Entry max sessions".to_string(),
                details: None,
            }];
        }

        let stats = self.price_stats.read().await;

        let mut ignores: Vec<StrategyAction> = vec![];
        if matches!(
            stats.long_trend,
            Some(PriceTrend::Crash) | Some(PriceTrend::Down)
        ) && !matches!(stats.short_trend, Some(PriceTrend::Bull))
        {
            ignores.push(StrategyAction::Ignore {
                ticker: self.ticker.clone(),
                reason: "Entry price down".to_string(),
                details: None,
            });
        }

        if stats.long_support.is_some_and(|v| v > current_buy_price) {
            ignores.push(StrategyAction::Ignore {
                ticker: self.ticker.clone(),
                reason: "Entry price > support".to_string(),
                details: None,
            });
        }

        if !ignores.is_empty() {
            return ignores;
        }

        let mut order = Order::new_buy(
            self.ticker.clone(),
            amount,
            price,
            amount * price,
            current_time,
            None,
        );

        match self
            .marketplace
            .adjust_order_price_and_amount(&mut order)
            .await
        {
            Ok(_) => vec![StrategyAction::PlaceOrder { order }],
            Err(err) => {
                error!("Failed to adjust order amount : {}", err);
                vec![]
            }
        }
    }

    async fn on_depth_event(
        &mut self,
        event: &MarketplaceBook,
        tx_app: Sender<AppEvent>,
    ) -> Result<()> {
        if self.ticker != event.ticker {
            return Ok(());
        }

        let current_buy_price = event
            .buy_price()
            .context(format!("Current buy price missing for {}.", event.ticker))?;

        let current_sell_price = event
            .sell_price()
            .context(format!("Current sell price missing for {}.", event.ticker))?;

        {
            let state = self.state.read().await;
            // if there is a pending order, wait for it to be processed
            if !state
                .find_by(
                    OrderListFilters {
                        ticker: Some(self.ticker.clone()),
                        status: vec![OrderStatus::Draft, OrderStatus::Sent, OrderStatus::Active],
                        ..Default::default()
                    },
                    OrderListSort {
                        by: OrderListSortBy::Date,
                        asc: true,
                    },
                )
                .is_empty()
            {
                return Ok(());
            }
        }

        let mut actions: Vec<StrategyAction> = vec![];

        actions.append(&mut self.process_sell(current_sell_price, event.time).await);

        actions.append(&mut self.process_reentry(current_buy_price, event.time).await);

        actions.append(&mut self.process_entry(current_buy_price, event.time).await);

        for action in actions {
            tx_app.send(AppEvent::Strategy(StrategyEvent::Action(action)))?;
        }

        Ok(())
    }
}

impl<M> Strategy for ScalpingStrategy<M>
where
    M: Marketplace + MarketplaceSettingsApi + MarketplaceDataApi,
{
    async fn start(&mut self, tx_app: Sender<AppEvent>) {
        let mut rx_app = tx_app.subscribe();
        loop {
            if let Ok(event) = rx_app.recv().await {
                match event {
                    AppEvent::MarketPlace(MarketplaceEvent::Candle(event)) => {
                        if self.ticker == event.ticker {
                            self.add_candle_event_history(event.clone()).await;
                        }
                    }
                    AppEvent::MarketPlace(MarketplaceEvent::Book(event)) => {
                        if self.ticker == event.ticker {
                            let _ = self.on_depth_event(&event, tx_app.clone()).await;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dequeue() {
        let mut v = VecDeque::new();
        v.push_front(1);
        v.push_front(2);
        v.push_front(3);

        assert_eq!([3, 2, 1], v.make_contiguous());

        let v2 = VecDeque::from(vec![3, 2, 1]);
        assert_eq!(v, v2);
    }
}
