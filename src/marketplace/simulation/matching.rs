use crate::marketplace::simulation::{SimulationMarketplace, SimulationSource};
use crate::marketplace::{MarketplaceMatching, MarketplaceOrderUpdate, MarketplaceSettingsApi};
use crate::order::{OrderSide, OrderStatus, OrderTrade, OrderType};
use crate::ticker::Ticker;
use crate::AppEvent;
use colored::Colorize;
use rust_decimal_macros::dec;
use std::time::Duration;
use tokio::sync::broadcast::Sender;
use tracing::{error, info};
use uuid::Uuid;

impl<S: MarketplaceSettingsApi> SimulationMarketplace<S> {
    async fn tick(&mut self) -> anyhow::Result<()> {
        match self.source {
            SimulationSource::Book => self.match_order_on_book().await?,
            _ => {}
        };
        Ok(())
    }

    async fn match_order_on_book(&self) -> anyhow::Result<()> {
        let time = { *self.current_time.read().await };

        let mut orders = self.orders.write().await;
        let book = self.order_book.read().await;

        for order in orders
            .iter_mut()
            .filter(|order| matches!(order.status, OrderStatus::Active))
        {
            if let Some(book) = book.get(&order.ticker) {
                let book = match order.side {
                    OrderSide::Sell => book.bids.iter(),
                    OrderSide::Buy => book.asks.iter(),
                };

                for book_order in book {
                    let trade = match order.order_type {
                        OrderType::Market => {
                            let to_fulfill_quote =
                                order.amount * order.price - order.get_trade_total_price();
                            if to_fulfill_quote <= dec!(0) {
                                break;
                            }
                            if to_fulfill_quote > book_order.0 * book_order.1 {
                                order.filled_amount += book_order.1;
                                order.cumulative_quote_amount += book_order.1 * book_order.0;
                                OrderTrade {
                                    id: Uuid::new_v4().to_string(),
                                    trade_time: time,
                                    amount: book_order.1,
                                    price: book_order.0,
                                }
                            } else {
                                let taken = to_fulfill_quote / book_order.0;
                                order.filled_amount += taken;
                                order.cumulative_quote_amount += taken * book_order.0;
                                order.status = OrderStatus::Executed;
                                OrderTrade {
                                    id: Uuid::new_v4().to_string(),
                                    trade_time: time,
                                    amount: taken,
                                    price: book_order.0,
                                }
                            }
                        }
                        OrderType::Limit => {
                            match order.side {
                                OrderSide::Buy => {
                                    if book_order.0 > order.price {
                                        break;
                                    }
                                }
                                OrderSide::Sell => {
                                    if book_order.0 < order.price {
                                        break;
                                    }
                                }
                            };

                            let to_fulfill = order.amount - order.filled_amount;
                            if to_fulfill <= dec!(0) {
                                break;
                            }
                            if to_fulfill > book_order.1 {
                                order.filled_amount += book_order.1;
                                order.cumulative_quote_amount += book_order.1 * book_order.0;
                                OrderTrade {
                                    id: Uuid::new_v4().to_string(),
                                    trade_time: time,
                                    amount: book_order.1,
                                    price: book_order.0,
                                }
                            } else {
                                order.filled_amount += to_fulfill;
                                order.cumulative_quote_amount += to_fulfill * book_order.0;
                                order.status = OrderStatus::Executed;
                                OrderTrade {
                                    id: Uuid::new_v4().to_string(),
                                    trade_time: time,
                                    amount: to_fulfill,
                                    price: book_order.0,
                                }
                            }
                        }
                        _ => {
                            continue;
                        }
                    };
                    order.trades.push(trade.clone());
                    self.notify_order_update(MarketplaceOrderUpdate {
                        time,
                        update_type: "TRADE".to_owned(),
                        marketplace_id: order.marketplace_id.clone().unwrap(),
                        client_id: order.id.to_owned(),
                        status: order.status.to_owned(),
                        working_time: Some(time),
                        trade: Some(trade.clone()),
                    })
                    .await;
                    info!(
                        " {} for order {} {}/{} : +{}",
                        match order.side {
                            OrderSide::Buy => "BUY".green(),
                            OrderSide::Sell => "SELL".red(),
                        },
                        Ticker::to_string(&order.ticker),
                        order.filled_amount,
                        order.amount,
                        trade.amount
                    );

                    let fees = self.settings.get_fees().await;

                    match order.side {
                        OrderSide::Sell => {
                            // increase portfolio quote asset
                            let added_quote_amount = trade.price * trade.amount * (dec!(1) - fees);
                            info!(" ADDED {} {}", added_quote_amount, order.ticker.quote);
                            self.update_asset_amount(
                                &order.ticker.quote,
                                added_quote_amount,
                                Some(dec!(1)),
                            )
                            .await;

                            // decrease portfolio base asset
                            let removed_base_amount = trade.amount;
                            info!(" REMOVED {} {}", removed_base_amount, order.ticker.base);
                            self.update_asset_amount(
                                &order.ticker.base,
                                -removed_base_amount,
                                Some(trade.price),
                            )
                            .await;
                        }
                        OrderSide::Buy => {
                            // increase portfolio base asset
                            let added_base_amount = trade.amount * (dec!(1) - fees);
                            info!(" ADDED {} {}", added_base_amount, order.ticker.base);
                            self.update_asset_amount(
                                &order.ticker.base,
                                added_base_amount,
                                Some(trade.price),
                            )
                            .await;

                            // decrease portfolio quote asset
                            // no fees, they apply to the bought asset
                            let removed_quote_amount = trade.price * trade.amount;
                            info!(" REMOVED {} {}", removed_quote_amount, order.ticker.quote);
                            self.update_asset_locked(
                                &order.ticker.quote,
                                -removed_quote_amount,
                                Some(dec!(1)),
                            )
                            .await;
                        }
                    };
                }
            }
        }

        Ok(())
    }
}

impl<S: MarketplaceSettingsApi> MarketplaceMatching for SimulationMarketplace<S> {
    async fn start_matching(&mut self, app_tx: Sender<AppEvent>) -> anyhow::Result<()> {
        let mut app_rx = app_tx.subscribe();

        let current_time = self.current_time.clone();
        let order_book = self.order_book.clone();
        let sync_time = async {
            loop {
                match app_rx.recv().await {
                    Ok(AppEvent::MarketPlace(event)) => {
                        if let Some(time) = event.get_time() {
                            let mut current_time = current_time.write().await;
                            *current_time = time;
                        }
                        match event {
                            crate::marketplace::MarketplaceEvent::Book(book) => {
                                let mut order_book = order_book.write().await;
                                order_book.insert(book.ticker.clone(), book);
                            }
                            _ => {}
                        }
                    }
                    Err(err) => {
                        error!("Failed recv : {err}");
                    }
                    _ => {}
                }
            }
        };

        let tick = async {
            let mut ticker = tokio::time::interval(Duration::from_millis(100));
            loop {
                ticker.tick().await;
                let _ = self.tick().await;
            }
        };

        tokio::join!(sync_time, tick);

        Ok(())
    }
}
