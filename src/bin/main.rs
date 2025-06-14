use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::Colorize;
use futures::future;
use futures_util::{SinkExt, StreamExt};
use marketplace::binance::Binance;
use marketplace::*;
use order::{OrderSide, OrderStatus, OrderTrade};
use portfolio::Asset;
use rust_decimal_macros::dec;
use state::State;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strategy::{scalping::ScalpingStrategy, Strategy};
use ticker::Ticker;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::select;
use tokio::sync::{watch, RwLock};
use tokio_tungstenite::connect_async;
use tracing::{debug, error, info, trace};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use trading_bot::marketplace::MarketPlaceStream;
use trading_bot::state::StateEvent;
use trading_bot::strategy::StrategyAction;
use tungstenite::Message;

use trading_bot::strategy::scalping::ScalpingParams;
use trading_bot::tui::app::App;
use trading_bot::*;

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    replay_path: Option<PathBuf>,
    #[arg(long)]
    store_path: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    AccountInfo,
    Start {
        #[arg(long, value_delimiter = ',', default_value = "BTCUSDT")]
        symbol: Vec<String>,
        #[arg(long, default_value = "127.0.0.1:5555")]
        server_address: String,
        #[arg(long)]
        replay_path: Option<PathBuf>,
        #[arg(long)]
        real: bool,
        #[arg(long, default_value = "USDT")]
        quote: String,
    },
    Replay {
        #[arg(long, value_delimiter = ',', default_value = "BTCUSDT")]
        symbol: Vec<String>,
        #[arg(long, default_value = "127.0.0.1:5554")]
        server_address: String,
        #[arg(long)]
        no_server: bool,
        #[arg(long)]
        replay_path: PathBuf,
        #[arg(long, default_value = "USDT")]
        quote: String,
        #[arg(long, default_value = "1")]
        interval: u64,
    },
    Tui {
        #[arg(long, default_value = "127.0.0.1:5555")]
        server_address: String,
        #[arg(long, default_value = "USDT")]
        quote: String,
    },
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            format!(
                "{}=debug,tower_http=debug,reqwest=debug",
                env!("CARGO_CRATE_NAME")
            )
            .into()
        }))
        .with(fmt::layer())
        .init();

    let args = Args::parse();

    match args.command {
        Some(Commands::AccountInfo) => {
            let _ = run_account_info().await;
        }
        Some(Commands::Start {
            quote,
            symbol,
            replay_path,
            server_address,
            real,
        }) => {
            let tickers: Vec<Ticker> = symbol
                .iter()
                .flat_map(|symbol| Ticker::try_from(symbol))
                .collect();
            let _ = run_start(quote, tickers, replay_path, server_address, real).await;
        }
        Some(Commands::Replay {
            interval,
            quote,
            symbol,
            replay_path,
            server_address,
            no_server,
        }) => {
            let tickers: Vec<Ticker> = symbol
                .iter()
                .flat_map(|symbol| Ticker::try_from(symbol))
                .collect();
            let _ = run_replay(
                interval,
                quote,
                tickers,
                replay_path,
                if no_server {
                    None
                } else {
                    Some(server_address)
                },
            )
            .await;
        }
        Some(Commands::Tui {
            quote,
            server_address,
        }) => {
            let _ = run_tui(quote, server_address).await;
        }
        _ => {}
    }
}

async fn run_account_info() -> Result<()> {
    let marketplace = Binance::new();
    let account_overview = marketplace.get_account_overview(true).await;
    info!("{:?}", account_overview);

    Ok(())
}

async fn run_tui(quote: String, server_address: String) -> Result<()> {
    let (tx, rx) = tokio::sync::mpsc::channel::<AppEvent>(100);
    let (tx_cmd, mut rx_cmd) = tokio::sync::mpsc::channel::<AppCommandEvent>(100);
    let mut app = App::new(rx, tx_cmd, quote);

    let ws_client_task = tokio::task::spawn(async move {
        // (re)connect loop
        loop {
            let stream;
            // wait for server
            loop {
                let url = format!("ws://{}/ws", server_address);
                if let Ok(res) = connect_async(url).await {
                    stream = res.0;
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }

            let tx = tx.clone();

            let (mut write, mut read) = stream.split();

            loop {
                select! {
                    msg = read.next() => {
                        match msg {
                            Some(Ok(Message::Text(msg))) => {
                                if let Ok(event) = serde_json::de::from_slice::<AppEvent>(msg.as_bytes()) {
                                    let _ = tx.send(event).await;
                                }
                            },
                        Some(Ok(Message::Close(_))) | None => {
                                    break;
                                }
                            _ => {}
                        }
                    }
                    Some(cmd) = rx_cmd.recv() => {
                        if let Ok(cmd) = serde_json::ser::to_string(&cmd) {
                            let _ = write.send(Message::Text(cmd.into())).await;
                        }
                    }
                }
            }

            // connection closed by server, wait before reconnecting
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });

    let app_task = tokio::task::spawn(async move {
        let _ = app.run().await;
    });

    select! {
        _ = app_task => {},
        _ = ws_client_task => {}
    }

    ratatui::restore();

    Ok(())
}

async fn run_start(
    quote: String,
    tickers: Vec<Ticker>,
    replay_path: Option<PathBuf>,
    server_address: String,
    real: bool,
) -> Result<()> {
    let state: Arc<RwLock<state::State>> = Arc::from(RwLock::from(state::State::new()));

    let (tx_app, _) = tokio::sync::broadcast::channel::<AppEvent>(1000);
    let (tx_cmd, _) = tokio::sync::mpsc::channel::<AppCommandEvent>(16);

    let mut marketplace = Binance::new();
    marketplace.init(&tickers).await?;

    {
        let state = state.clone();
        let mut state = state.write().await;
        if real {
            state.portfolio.assets = marketplace.get_account_assets().await?;
        } else {
            state.portfolio.assets = tickers
                .iter()
                .map(|ticker| {
                    (
                        ticker.base.clone(),
                        Asset {
                            symbol: ticker.base.clone(),
                            amount: dec!(0),
                            locked: dec!(0),
                            value: None,
                        },
                    )
                })
                .collect();
            state.portfolio.assets.insert(
                quote.clone(),
                Asset {
                    symbol: quote.clone(),
                    amount: dec!(5000),
                    locked: dec!(5000),
                    value: None,
                },
            );
        }
    }

    for ticker in tickers.iter() {
        let mut strategy = ScalpingStrategy::new(
            state.clone(),
            marketplace.clone(),
            ticker.clone(),
            ScalpingParams {
                target_profit: dec!(3),
                quote_amount: dec!(300),
                buy_cooldown: Duration::from_secs(60 * 15),
                multiple_orders: true,
            },
        );
        if strategy.init(None).await.is_err() {
            panic!("Could not init strategy");
        }

        tokio::task::spawn({
            let state = state.clone();
            let tx_app = tx_app.clone();
            let mut rx = tx_app.subscribe();

            async move {
                loop {
                    let event = rx.recv().await;
                    if let Ok(AppEvent::MarketPlace(event)) = event {
                        match strategy.on_marketplace_event(event).await {
                            Ok(actions) => {
                                for action in actions {
                                    process_strategy_action(state.clone(), action, tx_app.clone())
                                        .await;
                                }
                            }
                            Err(_) => {}
                        }
                    }
                }
            }
        });
    }

    match replay_path {
        Some(replay_path) => tokio::task::spawn({
            let tx_app = tx_app.clone();
            let mut rx = tx_app.subscribe();

            async move {
                let mut file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(replay_path)
                    .await
                    .expect("Failed to open file for storing events");

                loop {
                    // wait for a marketplace event
                    let event = rx.recv().await;
                    if let Ok(AppEvent::MarketPlace(event)) = event {
                        match serde_json::ser::to_string(&event) {
                            Ok(json) => {
                                file.write_all(json.as_bytes())
                                    .await
                                    .expect("Failed to write to store file");
                                file.write_all(b"\n")
                                    .await
                                    .expect("Failed to write to store file");
                            }
                            Err(err) => {
                                error!("Failed to save event : {}", err);
                            }
                        }
                    }
                }
            }
        }),
        None => tokio::task::spawn({
            let future = future::pending();
            async move {
                let () = future.await;
            }
        }),
    };

    tokio::task::spawn({
        let state = state.clone();
        let tx_app = tx_app.clone();
        async move {
            simulate_new_orders_processing(state, tx_app, false).await;
        }
    });

    tokio::task::spawn({
        let tx_app = tx_app.clone();
        let state = state.clone();
        let marketplace = marketplace.clone();

        async move {
            simulate_orders_processing(state, marketplace, tx_app, false).await;
        }
    });

    tokio::task::spawn({
        let state = state.clone();
        let tx_app = tx_app.clone();
        async move {
            update_portfolio_value(state, tx_app).await;
        }
    });

    tokio::task::spawn({
        let state = state.clone();
        async move { print_overview(state).await }
    });

    let marketplace_task = tokio::task::spawn({
        let mut marketplace = marketplace.clone();
        let tx_app = tx_app.clone();
        let tickers = tickers.clone();

        async move {
            marketplace.start(&tickers, tx_app).await;
        }
    });

    let server_task = tokio::task::spawn({
        let state = state.clone();
        let tx_app = tx_app.clone();
        let tx_cmd = tx_cmd.clone();
        async move { server::start(server_address, state, tx_app, tx_cmd).await }
    });

    info!("{}", "STARTING BOT".green());

    tokio::select! {
        _ = marketplace_task => {}
        _ = server_task => {}
    }

    Ok(())
}

async fn run_replay(
    interval: u64,
    quote: String,
    tickers: Vec<Ticker>,
    replay_path: PathBuf,
    server_address: Option<String>,
) -> Result<()> {
    let state: Arc<RwLock<state::State>> = Arc::from(RwLock::from(state::State::new()));

    let (tx_app, _) = tokio::sync::broadcast::channel::<AppEvent>(10000);
    let (tx_cmd, mut rx_cmd) = tokio::sync::mpsc::channel::<AppCommandEvent>(16);

    let mut marketplace = Binance::new();
    marketplace.init(&tickers).await?;

    {
        let state = state.clone();
        let mut state = state.write().await;
        state.portfolio.assets = tickers
            .iter()
            .map(|ticker| {
                (
                    ticker.base.clone(),
                    Asset {
                        symbol: ticker.base.clone(),
                        amount: dec!(0),
                        locked: dec!(0),
                        value: None,
                    },
                )
            })
            .collect();
        state.portfolio.assets.insert(
            quote.clone(),
            Asset {
                symbol: quote.clone(),
                amount: dec!(1000),
                locked: dec!(0),
                value: None,
            },
        );
    }

    let mut start_time: Option<u64> = None;
    {
        let file = File::open(replay_path.clone())
            .await
            .expect("Failed to open replay file");
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            match serde_json::de::from_str::<MarketPlaceEvent>(&line) {
                Ok(event) => match event {
                    MarketPlaceEvent::Depth(event) => {
                        start_time = Some(event.time);
                        break;
                    }
                    _ => {}
                },
                Err(err) => {
                    error!("Failed parsing replay line : {}", err);
                    break;
                }
            }
        }
    }
    if start_time.is_none() {
        panic!("Could not find start time from replay file");
    }

    for ticker in tickers.iter() {
        let mut strategy = ScalpingStrategy::new(
            state.clone(),
            marketplace.clone(),
            ticker.clone(),
            ScalpingParams {
                target_profit: dec!(1.5),
                quote_amount: dec!(100),
                buy_cooldown: Duration::from_secs(60 * 1),
                multiple_orders: true,
            },
        );
        if strategy.init(start_time).await.is_err() {
            panic!("Could not init strategy");
        }

        tokio::task::spawn({
            let state = state.clone();
            let tx_app = tx_app.clone();
            let mut rx = tx_app.subscribe();

            async move {
                loop {
                    // wait for a marketplace event
                    match rx.recv().await {
                        Ok(AppEvent::MarketPlace(event)) => {
                            debug!("MarketPlace event : {:?}", event);
                            match strategy.on_marketplace_event(event).await {
                                Ok(actions) => {
                                    for action in actions {
                                        process_strategy_action(
                                            state.clone(),
                                            action,
                                            tx_app.clone(),
                                        )
                                        .await;
                                    }
                                }
                                Err(err) => {
                                    error!("Failed to process marketplace event : {}", err)
                                }
                            }
                        }
                        Err(err) => {
                            error!("Receive event error : {}", err);
                        }
                        Ok(other) => {
                            debug!("AppEvent : {:?}", other);
                        }
                    }
                }
            }
        });
    }

    tokio::task::spawn({
        let state = state.clone();
        let tx_app = tx_app.clone();
        async move {
            simulate_new_orders_processing(state, tx_app, true).await;
        }
    });

    tokio::task::spawn({
        let tx_app = tx_app.clone();
        let state = state.clone();
        let marketplace = marketplace.clone();

        async move {
            simulate_orders_processing(state, marketplace, tx_app, true).await;
        }
    });

    tokio::task::spawn({
        let state = state.clone();
        let tx_app = tx_app.clone();
        async move {
            update_portfolio_value(state, tx_app).await;
        }
    });

    tokio::task::spawn({
        let state = state.clone();
        async move { print_overview(state).await }
    });

    let (pause_tx, pause_rx) = watch::channel(false);
    tokio::task::spawn({
        let pause_tx = pause_tx.clone();
        async move {
            loop {
                if let Some(cmd) = rx_cmd.recv().await {
                    info!("Got command {:?}", cmd);
                    match cmd {
                        AppCommandEvent::Pause => {
                            let new_state = !*pause_tx.borrow();
                            pause_tx.send(new_state).unwrap();
                        }
                    }
                }
            }
        }
    });

    let marketplace_task = tokio::task::spawn({
        let tx_app = tx_app.clone();
        let mut pause_rx = pause_rx.clone();

        async move {
            let file = File::open(replay_path)
                .await
                .expect("Failed to open replay file");
            let reader = BufReader::new(file);
            let mut lines = reader.lines();

            let mut i = 0;
            while let Ok(Some(line)) = lines.next_line().await {
                while *pause_rx.borrow() {
                    pause_rx.changed().await.unwrap();
                }
                if let Ok(event) = serde_json::de::from_str(&line) {
                    tx_app.send(AppEvent::MarketPlace(event)).unwrap();
                    i += 1;

                    if interval > 0 {
                        tokio::time::sleep(Duration::from_micros(interval)).await;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });

    let server_task = match server_address {
        Some(server_address) => tokio::task::spawn({
            let state = state.clone();
            let tx_app = tx_app.clone();
            let tx_cmd = tx_cmd.clone();
            async move {
                let _ = server::start(server_address, state, tx_app, tx_cmd).await;
            }
        }),
        None => tokio::task::spawn({
            let future = future::pending();
            async move {
                let () = future.await;
            }
        }),
    };

    info!("{}", "STARTING REPLAY".green());

    tokio::select! {
        _ = marketplace_task => { info!("marketplace_task ended"); }
        _ = server_task => { info!("server_task ended"); }
    }

    Ok(())
}

async fn simulate_new_orders_processing(
    state: Arc<RwLock<State>>,
    tx_app: tokio::sync::broadcast::Sender<AppEvent>,
    is_replay: bool,
) {
    loop {
        // get simulation time
        let time = {
            let mut rx = tx_app.subscribe();
            let event = rx.recv().await;
            if let Ok(AppEvent::MarketPlace(MarketPlaceEvent::Depth(event))) = event {
                event.time
            } else {
                continue;
            }
        };

        let mut state = state.write().await;
        let State { orders, portfolio } = &mut *state;

        let mut processed = false;

        for order in orders
            .iter_mut()
            .filter(|order| matches!(order.status, OrderStatus::Sent))
        {
            if order.creation_time + 2000_u64 < time {
                match portfolio.reserve_funds(
                    match order.side {
                        OrderSide::Buy => &order.ticker.quote,
                        OrderSide::Sell => &order.ticker.base,
                    },
                    match order.side {
                        OrderSide::Buy => order.amount * order.price,
                        OrderSide::Sell => order.amount,
                    },
                ) {
                    Ok(()) => {}
                    Err(err) => {
                        order.status = OrderStatus::Rejected;
                        processed = true;
                        continue;
                    }
                }
                order.status = OrderStatus::Active;
                order.working_time = Some(time);
            }
            processed = true;
        }

        if processed {
            let _ = tx_app.send(AppEvent::State(StateEvent::Orders(orders.clone())));
        }

        processed = false;
        for order in orders
            .iter_mut()
            .filter(|order| order.status == OrderStatus::Draft)
        {
            // simulate sending request to markeplace
            if !is_replay {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            order.status = OrderStatus::Sent;
            processed = true;
        }

        if processed {
            let _ = tx_app.send(AppEvent::State(StateEvent::Orders(orders.clone())));
        }
    }
}

// listen to public trade events
// when a trade is executed, fill (partially or fully) a matching active order
async fn simulate_orders_processing<M>(
    state: Arc<RwLock<State>>,
    marketplace: M,
    tx_app: tokio::sync::broadcast::Sender<AppEvent>,
    is_replay: bool,
) where
    M: MarketPlace + MarketPlaceTrade + MarketPlaceSettings,
{
    let mut rx = tx_app.subscribe();
    loop {
        let event = rx.recv().await;
        if let Ok(AppEvent::MarketPlace(MarketPlaceEvent::Depth(event))) = event {
            let mut processed = false;
            let mut state = state.write().await;

            let State {
                portfolio, orders, ..
            } = &mut *state;

            // find active order
            for order in orders.iter_mut().filter(|order| {
                matches!(order.status, OrderStatus::Active) && order.ticker == event.ticker
            }) {
                // simulate if a trade will eat part of the order
                let trade_amount = match order.order_type {
                    order::OrderType::Market => match order.side {
                        OrderSide::Sell => event.bids.first().clone(),
                        OrderSide::Buy => event.asks.first().clone(),
                    },
                    order::OrderType::Limit => {
                        match order.side {
                            // sell order : if the trade price >= order price, it will be eaten
                            OrderSide::Sell => event.bids.first().and_then(|bid| {
                                if bid.0 >= order.price {
                                    Some(bid)
                                } else {
                                    None
                                }
                            }),
                            // buy order : if the trade price <= order price, it will be eaten
                            OrderSide::Buy => event.asks.first().and_then(|ask| {
                                if ask.0 <= order.price {
                                    Some(ask)
                                } else {
                                    None
                                }
                            }),
                        }
                    }
                    _ => None,
                };

                if trade_amount.is_none() {
                    trace!("No trade for order");
                    continue;
                }
                let trade_amount = trade_amount.unwrap();

                let to_fulfill = order.amount - order.filled_amount;
                //let to_fulfill_quote = order.amount * order.price - order.get_trade_total_price();

                let order_trade = if to_fulfill > trade_amount.1 {
                    order.filled_amount += trade_amount.1;
                    info!(
                        " {} for order {} {}/{} : +{}",
                        match order.side {
                            OrderSide::Buy => "PARTIAL BUY".green(),
                            OrderSide::Sell => "PARTIAL SELL".red(),
                        },
                        Ticker::to_string(&event.ticker),
                        order.filled_amount,
                        order.amount,
                        trade_amount.1
                    );
                    OrderTrade {
                        trade_time: event.time,
                        amount: trade_amount.1,
                        price: trade_amount.0,
                    }
                } else {
                    order.status = OrderStatus::Executed;
                    order.filled_amount = order.amount;
                    info!(
                        " {} for order {} {}/{} : +{}",
                        match order.side {
                            OrderSide::Buy => "FINAL BUY".green(),
                            OrderSide::Sell => "FINAL SELL".red(),
                        },
                        Ticker::to_string(&event.ticker),
                        order.filled_amount,
                        order.amount,
                        to_fulfill
                    );
                    OrderTrade {
                        trade_time: event.time,
                        amount: to_fulfill,
                        price: trade_amount.0,
                    }
                };

                order.trades.push(order_trade.clone());

                let fees = marketplace.get_fees().await;

                let price = trade_amount.0;

                match order.side {
                    OrderSide::Sell => {
                        // increase portfolio quote asset
                        let added_quote_amount = price * order_trade.amount * (dec!(1) - fees);
                        info!(" ADDED {} {}", added_quote_amount, order.ticker.quote);
                        portfolio.update_asset_amount(
                            &order.ticker.quote,
                            added_quote_amount,
                            dec!(1),
                        );

                        // decrease portfolio base asset
                        let removed_base_amount = order_trade.amount;
                        info!(" REMOVED {} {}", removed_base_amount, order.ticker.base);
                        portfolio.drain_asset_locked(
                            &order.ticker.base,
                            removed_base_amount,
                            price,
                        );
                    }
                    OrderSide::Buy => {
                        // increase portfolio base asset
                        let added_base_amount = order_trade.amount * (dec!(1) - fees);
                        info!(" ADDED {} {}", added_base_amount, order.ticker.base);
                        portfolio.update_asset_amount(&order.ticker.base, added_base_amount, price);

                        // decrease portfolio quote asset
                        // no fees, they apply to the bought asset
                        let removed_quote_amount = price * order_trade.amount;
                        info!(" REMOVED {} {}", removed_quote_amount, order.ticker.quote);
                        portfolio.drain_asset_locked(
                            &order.ticker.quote,
                            removed_quote_amount,
                            dec!(1),
                        );
                    }
                }
                processed = true;
            }
            if !is_replay {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if processed {
                info!("{}", state.portfolio,);
                for (symbol, _) in &state.portfolio.assets {
                    info!(
                        "{} scalped : {}",
                        symbol,
                        state.get_total_scalped(symbol.clone())
                    );
                }
                let _ = tx_app.send(AppEvent::State(StateEvent::Portfolio(
                    state.portfolio.clone(),
                )));
                let _ = tx_app.send(AppEvent::State(StateEvent::Orders(state.orders.clone())));
            }
        }
    }
}

async fn update_portfolio_value(
    state: Arc<RwLock<State>>,
    tx_app: tokio::sync::broadcast::Sender<AppEvent>,
) {
    let mut rx = tx_app.subscribe();
    loop {
        let event = rx.recv().await;
        if let Ok(AppEvent::MarketPlace(MarketPlaceEvent::Depth(event))) = event {
            let mut state = state.write().await;
            if let Some(asset) = state.portfolio.assets.get_mut(&event.ticker.base) {
                if let Some(buy_price) = event.buy_price() {
                    asset.value = Some((asset.amount + asset.locked) * buy_price);
                    state.portfolio.update_value();
                }
            }
        }
    }
}

async fn print_overview(state: Arc<RwLock<State>>) {
    loop {
        {
            let state = state.read().await;
            info!("{}", state.portfolio,);
            for (symbol, _) in &state.portfolio.assets {
                info!(
                    "{} scalped : {}",
                    symbol,
                    state.get_total_scalped(symbol.clone())
                );
            }
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

async fn process_strategy_action(
    state: Arc<RwLock<State>>,
    action: StrategyAction,
    tx: tokio::sync::broadcast::Sender<AppEvent>,
) {
    match &action {
        StrategyAction::PlaceOrder { order } => {
            info!("{:?}", order);
            let mut state = state.write().await;
            let _ = state.add_order(order.clone());
        }
        _ => {}
    }
    let _ = tx.send(AppEvent::Strategy(strategy::StrategyEvent::Action(action)));
}
