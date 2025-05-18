use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::{collections::HashMap, fmt::Display};

#[derive(Clone, Debug)]
pub enum OrderType {
    Sell,
    Buy,
}

#[derive(Clone, Debug)]
pub struct Order {
    pub order_type: OrderType,
    pub amount: Decimal,
    pub price: Decimal,
}

pub struct Position {
    pub amount: Decimal,
}

pub struct State {
    pub bank: Decimal,
    pub positions: HashMap<String, Position>,
    pub orders: HashMap<String, Order>,
    pub order_history: HashMap<String, Order>,
    pub market_history: HashMap<String, Vec<Decimal>>,
}

impl State {
    pub fn new() -> Self {
        State {
            bank: dec!(100000),
            positions: HashMap::new(),
            orders: HashMap::new(),
            order_history: HashMap::new(),
            market_history: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Tick {
    pub symbol: String,
    pub price: Decimal,
}

impl Display for Tick {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.symbol, self.price)
    }
}

pub fn tick(state: &mut State, tick: Tick) {
    //println!("-- tick -- {}", tick);
    if let Some(market_history) = state.market_history.get_mut(&tick.symbol) {
        market_history.push(tick.price);
        match (
            state.positions.get_mut(&tick.symbol),
            state.orders.get(&tick.symbol),
        ) {
            (Some(position), None) => {
                if let Some(sell_order) = decide_sell(
                    state.bank,
                    &market_history,
                    state.order_history.get(&tick.symbol),
                    &position,
                    tick.price,
                ) {
                    state.orders.insert(tick.symbol.clone(), sell_order.clone());
                    println!(
                        "-- sell order: {} {} for {} --",
                        sell_order.amount, tick.symbol, sell_order.price
                    );
                    position.amount = position.amount - sell_order.amount;
                    if position.amount <= dec!(0) {
                        state.positions.remove(&tick.symbol);
                    }
                }
            }
            (_, None) => {
                if let Some(buy_order) = decide_buy(state.bank, &market_history, tick.price) {
                    let new_bank = state.bank - buy_order.price;

                    if new_bank > dec!(0) {
                        state.orders.insert(tick.symbol.clone(), buy_order.clone());
                        state.bank = state.bank - buy_order.price;
                        println!(
                            "-- buy order: {} {} for {} --",
                            buy_order.amount, tick.symbol, buy_order.price
                        );
                    }
                }
            }
            _ => {}
        }
    } else {
        state
            .market_history
            .insert(tick.symbol.clone(), vec![tick.price]);
    }
}

fn decide_buy(bank: Decimal, market_history: &Vec<Decimal>, price: Decimal) -> Option<Order> {
    if market_history.len() < 5 {
        return None;
    }
    Some(Order {
        order_type: OrderType::Buy,
        price: dec!(1000),
        amount: dec!(1000) / price,
    })
}

fn decide_sell(
    bank: Decimal,
    market_history: &Vec<Decimal>,
    order: Option<&Order>,
    position: &Position,
    price: Decimal,
) -> Option<Order> {
    if let Some(last_buy_order) = order {
        if let OrderType::Buy = last_buy_order.order_type {
            let profit = price * last_buy_order.amount - last_buy_order.price;
            let sell_price = price * last_buy_order.amount;
            if profit > dec!(5) + sell_price * dec!(0.001) {
                return Some(Order {
                    order_type: OrderType::Sell,
                    price: sell_price,
                    amount: last_buy_order.amount,
                });
            }
        }
    }
    None
}
