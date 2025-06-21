use std::time::Duration;
use tracing::{error, info};
use uuid::Uuid;

use super::SimulationMarketplace;
use crate::marketplace::{MarketplaceOrderUpdate, MarketplaceSettingsApi};
use crate::order::OrderSide;
use crate::ticker::Ticker;
use crate::{
    marketplace::MarketplaceTradeApi,
    order::{Order, OrderStatus},
};

impl<S: MarketplaceSettingsApi> MarketplaceTradeApi for SimulationMarketplace<S> {
    async fn get_orders(&self, tickers: &[Ticker]) -> anyhow::Result<Vec<Order>> {
        tokio::time::sleep(Duration::from_secs(1)).await;

        let orders = self.orders.read().await;
        let orders: Vec<Order> = orders
            .iter().filter(|&order| tickers.contains(&order.ticker)).cloned()
            .collect();
        Ok(orders.clone())
    }

    async fn place_order(&mut self, order: &Order) -> anyhow::Result<Order> {
        tokio::time::sleep(Duration::from_secs(1)).await;

        let time = { *self.current_time.read().await };

        let mut order = order.clone();

        let amount_to_reserve = match order.side {
            OrderSide::Buy => order.amount * order.price,
            OrderSide::Sell => order.amount,
        };
        let asset_to_reserve = match order.side {
            OrderSide::Buy => &order.ticker.quote,
            OrderSide::Sell => &order.ticker.base,
        };

        match self.lock_funds(asset_to_reserve, amount_to_reserve).await {
            Ok(()) => {
                info!(" RESERVED {} {}", amount_to_reserve, asset_to_reserve);
                order.working_time = Some(time);
                order.status = OrderStatus::Active;
            }
            Err(err) => {
                error!("Lock funds failed : {}", err);
                order.status = OrderStatus::Rejected;
            }
        }

        order.marketplace_id = Some(Uuid::new_v4().to_string());

        let mut orders = self.orders.write().await;
        orders.push(order.clone());

        self.notify_order_update(MarketplaceOrderUpdate {
            time,
            update_type: "NEW".to_owned(),
            marketplace_id: order.marketplace_id.as_ref().unwrap().clone(),
            client_id: order.id.clone(),
            status: order.status.clone(),
            working_time: None,
            trade: None,
        })
        .await;

        Ok(order)
    }
}
