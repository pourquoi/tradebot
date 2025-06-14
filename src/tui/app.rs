use anyhow::Result;
use chrono::DateTime;
use crossterm::event::{Event, EventStream, KeyCode};
use futures_util::StreamExt;
use ratatui::{
    layout::{
        Constraint::{self},
        Layout, Margin, Rect,
    },
    style::{palette::tailwind, Color, Style, Stylize},
    symbols,
    text::{Line, Span, Text},
    widgets::{
        Axis, Block, Borders, Cell, Chart, Dataset, List, ListItem, Paragraph, Row, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Sparkline, Table, TableState, Widget, Wrap,
    },
    Frame,
};
use rust_decimal::{prelude::ToPrimitive, Decimal};
use rust_decimal_macros::dec;
use std::{
    collections::{HashMap, VecDeque},
    time::Duration,
};
use tokio::sync::mpsc::{self, Receiver};

use crate::{
    marketplace::{CandleEvent, MarketPlaceEvent, TradeEvent},
    order::{Order, OrderSide, OrderTrade},
    portfolio::{Asset, Portfolio},
    state::StateEvent,
    strategy::StrategyEvent,
    ticker::Ticker,
    utils::avg,
    AppCommandEvent, AppEvent,
};

enum Window {
    None,
    Portfolio,
    Orders,
}

pub struct App {
    should_quit: bool,
    quote: String,
    rx: Receiver<AppEvent>,
    tx: mpsc::Sender<AppCommandEvent>,
    portfolio: Portfolio,
    orders: Vec<Order>,
    last_strategy_events: HashMap<Ticker, HashMap<String, (u64, String)>>,
    candles: HashMap<Ticker, VecDeque<CandleEvent>>,
    trades: HashMap<Ticker, VecDeque<TradeEvent>>,
    selected_window: Window,
    selected_asset: Option<String>,
    order_table_state: TableState,
    orders_scroll_state: ScrollbarState,
    orders_scroll: usize,
}

impl App {
    pub fn new(rx: Receiver<AppEvent>, tx: mpsc::Sender<AppCommandEvent>, quote: String) -> Self {
        Self {
            should_quit: false,
            rx,
            tx,
            quote,
            last_strategy_events: HashMap::new(),
            candles: HashMap::new(),
            trades: HashMap::new(),
            portfolio: Portfolio::new(),
            orders: Vec::new(),
            selected_asset: None,
            selected_window: Window::Portfolio,
            order_table_state: TableState::default(),
            orders_scroll_state: ScrollbarState::new(0),
            orders_scroll: 0,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        let mut terminal = ratatui::init();
        let _ = terminal.clear();

        let mut events = EventStream::new();

        let period = Duration::from_secs_f64(1.0 / 60.0);
        let mut interval = tokio::time::interval(period);

        while !self.should_quit {
            tokio::select! {
                _ = interval.tick() => { terminal.draw(|frame| self.render(frame))?; },
                Some(Ok(event)) = events.next() => self.handle_events(event).await,
                Some(event) = self.rx.recv() =>
                    self.handle_app_events(event)
            }
        }

        Ok(())
    }

    fn handle_app_events(&mut self, event: AppEvent) {
        match event {
            AppEvent::State(StateEvent::Portfolio(portfolio)) => {
                self.portfolio = portfolio.clone();
            }
            AppEvent::State(StateEvent::Orders(orders)) => {
                self.orders = orders;
                self.update_orders_scroll();
            }
            AppEvent::Strategy(StrategyEvent::Action(action)) => match action {
                crate::strategy::StrategyAction::Ignore {
                    ticker,
                    reason,
                    details,
                } => {
                    let entry = self.last_strategy_events.entry(ticker).or_default();
                    entry
                        .entry(reason)
                        .and_modify(|e| {
                            e.0 += 1;
                            if let Some(details) = details.clone() {
                                e.1 = details;
                            }
                        })
                        .or_insert((0, details.unwrap_or(String::new())));
                }
                _ => {}
            },
            AppEvent::MarketPlace(MarketPlaceEvent::Candle(candle)) => {
                let candles = self.candles.entry(candle.ticker.clone()).or_default();
                if let Some(last) = candles.pop_back() {
                    if last.start_time != candle.start_time {
                        candles.push_back(last);
                    }
                }
                candles.push_back(candle.clone());
                if candles.len() > 24 * 60 {
                    candles.pop_front();
                }
            }
            AppEvent::MarketPlace(MarketPlaceEvent::Trade(trade)) => {
                let trades = self
                    .trades
                    .entry(trade.ticker.clone())
                    .or_insert(VecDeque::with_capacity(300));
                trades.push_back(trade.clone());
                if trades.len() >= 300 {
                    trades.pop_front();
                }
            }
            _ => {}
        }
    }

    async fn handle_events(&mut self, event: Event) {
        if let Some(key) = event.as_key_press_event() {
            match key.code {
                KeyCode::Esc => match self.selected_window {
                    Window::None => self.should_quit = true,
                    Window::Orders => match self.get_selected_order() {
                        Some(order) => {
                            self.order_table_state.select(None);
                        }
                        _ => {
                            self.selected_window = Window::None;
                        }
                    },
                    _ => {
                        self.selected_asset = None;
                        self.selected_window = Window::None;
                    }
                },
                KeyCode::Up => match self.selected_window {
                    Window::Portfolio => {
                        self.selected_asset = self
                            .portfolio
                            .next_prev_symbol(self.selected_asset.clone(), false);
                    }
                    Window::Orders => {
                        self.orders_scroll = self.orders_scroll.saturating_sub(1);
                        self.orders_scroll_state =
                            self.orders_scroll_state.position(self.orders_scroll);
                        self.order_table_state.select(Some(self.orders_scroll));
                    }
                    _ => {}
                },
                KeyCode::Down => match self.selected_window {
                    Window::Portfolio => {
                        self.selected_asset = self
                            .portfolio
                            .next_prev_symbol(self.selected_asset.clone(), true);
                    }
                    Window::Orders => {
                        self.orders_scroll = self.orders_scroll.saturating_add(1);
                        self.orders_scroll_state =
                            self.orders_scroll_state.position(self.orders_scroll);
                        self.order_table_state.select(Some(self.orders_scroll));
                    }
                    _ => {}
                },
                KeyCode::Char('1') => {
                    self.selected_window = Window::Portfolio;
                    self.update_orders_scroll();
                }
                KeyCode::Char('2') => {
                    self.selected_window = Window::Orders;
                    self.update_orders_scroll();
                }
                KeyCode::Char('p') => {
                    let _ = self.tx.send(AppCommandEvent::Pause).await;
                }
                _ => {}
            }
        }
    }

    fn update_orders_scroll(&mut self) {
        let l = match self
            .selected_asset
            .as_ref()
            .and_then(|s| self.portfolio.assets.get(s))
        {
            Some(asset) => self
                .orders
                .iter()
                .filter(|order| order.ticker.base == asset.symbol)
                .count(),
            None => self.orders.len(),
        };
        self.orders_scroll_state = self.orders_scroll_state.content_length(l);
        if self.orders_scroll >= l {
            self.orders_scroll = 0;
        }
        self.order_table_state.select(Some(self.orders_scroll));
        //self.order_table_state = self
        //    .order_table_state
        //    .clone()
        //    .with_offset(self.orders_scroll);
    }

    fn get_orders(&self) -> Vec<&Order> {
        self.orders
            .iter()
            .filter(|order| {
                self.selected_asset
                    .as_ref()
                    .map(|selected_asset| &order.ticker.base == selected_asset)
                    .unwrap_or(true)
            })
            .collect()
    }

    fn get_selected_order(&self) -> Option<&Order> {
        match (&self.selected_window, self.order_table_state.selected()) {
            (Window::Orders, Some(pos)) => self.get_orders().get(pos).copied(),
            _ => None,
        }
    }

    fn render(&mut self, frame: &mut Frame) {
        let [header_area, main_area, footer_area] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Fill(1),
            Constraint::Length(3),
        ])
        .areas(frame.area());

        let [left_area, right_area] =
            Layout::horizontal([Constraint::Max(40), Constraint::Fill(1)]).areas(main_area);

        let [top_left_area, bottom_left_area] =
            Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).areas(left_area);

        let [top_right_area, bottom_right_area] =
            Layout::vertical([Constraint::Percentage(60), Constraint::Fill(1)]).areas(right_area);

        self.render_header(frame, header_area);
        self.render_footer(frame, footer_area);
        self.render_portfolio(frame, top_left_area);

        self.render_candles(frame, top_right_area);
        //self.render_trades(frame, middle_left_area);

        self.render_strategy_log(frame, bottom_left_area);

        self.render_detail(frame, bottom_right_area);
    }

    fn render_portfolio(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(format!(
                "(1) Portfolio $ {}",
                self.portfolio.value.unwrap_or(dec!(0)).round_dp(2)
            ))
            .borders(Borders::ALL)
            .border_style(match self.selected_window {
                Window::Portfolio => tailwind::BLUE.c500,
                _ => Color::White,
            });
        let mut assets: Vec<&String> = self.portfolio.assets.keys().collect();
        assets.sort();

        let items: Vec<ListItem> = assets
            .iter()
            .flat_map(|symbol| {
                self.portfolio.assets.get(*symbol).map(|asset| {
                    let mut item = ListItem::from(asset);
                    if let Some(selected_asset) = &self.selected_asset {
                        if selected_asset == *symbol {
                            item = item.bg(tailwind::BLUE.c500);
                        }
                    }
                    item
                })
            })
            .collect();
        let list = List::new(items).block(block);
        frame.render_widget(list, area);
    }

    fn render_detail(&mut self, frame: &mut Frame, area: Rect) {
        if let Some(order) = self.get_selected_order() {
            let [right_area, left_area] =
                Layout::horizontal([Constraint::Fill(1), Constraint::Max(40)]).areas(area);
            self.render_orders(frame, right_area);
            self.render_order_trades(frame, left_area);
        } else {
            self.render_orders(frame, area);
        }
    }

    fn render_orders(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title("(2) Orders")
            .borders(Borders::ALL)
            .border_style(match self.selected_window {
                Window::Orders => tailwind::BLUE.c500,
                _ => Color::White,
            });

        let header_style = Style::default()
            .fg(tailwind::SLATE.c900)
            .bg(tailwind::SLATE.c200);

        let header = [
            "#",
            "Date (UTC)",
            "Ticker",
            "Type",
            "Status",
            "Value",
            "Fullfilled",
            "Price",
            "Amount",
        ]
        .into_iter()
        .map(Cell::from)
        .collect::<Row>()
        .style(header_style)
        .height(1);

        let rows: Vec<Row> = self
            .get_orders()
            .into_iter()
            .enumerate()
            .map(|(i, order)| {
                Row::new([
                    Cell::from(format!("{}", order.session_id.clone().unwrap_or_default())),
                    Cell::from(
                        match order
                            .working_time
                            .and_then(|d| DateTime::from_timestamp_millis(d as i64))
                        {
                            Some(date) => format!("{}", date),
                            _ => "-".to_string(),
                        },
                    ),
                    Cell::from(format!("{}", order.ticker.base)).style(tailwind::BLUE.c500),
                    Cell::from(format!("{}", order.side)).style(Style::new().fg(
                        match order.side {
                            OrderSide::Buy => tailwind::GREEN.c500,
                            OrderSide::Sell => tailwind::RED.c500,
                        },
                    )),
                    Cell::from(format!("{}", order.status)),
                    Cell::from(format!("{}", order.price * order.amount)),
                    Cell::from(format!(
                        "{}%",
                        (dec!(100) * order.filled_amount / order.amount).round_dp(2)
                    )),
                    Cell::from(format!("{}", order.price)),
                    Cell::from(format!("{}", order.amount)),
                ])
                .style(match self.selected_window {
                    Window::Orders if i == self.orders_scroll => {
                        Style::new().bg(tailwind::BLUE.c500)
                    }
                    _ => match i % 2 {
                        0 => Style::new().bg(tailwind::SLATE.c950),
                        _ => Style::new().bg(tailwind::SLATE.c900),
                    },
                })
            })
            .collect();

        let t = Table::new(
            rows,
            [
                Constraint::Min(3),
                Constraint::Min(19),
                Constraint::Min(6),
                Constraint::Min(6),
                Constraint::Fill(1),
                Constraint::Fill(1),
                Constraint::Fill(1),
                Constraint::Fill(1),
                Constraint::Fill(1),
            ],
        )
        .header(header)
        .block(block);

        frame.render_stateful_widget(t, area, &mut self.order_table_state);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓")),
            area.inner(Margin {
                vertical: 1,
                horizontal: 1,
            }),
            &mut self.orders_scroll_state,
        );
    }

    fn render_order_trades(&self, frame: &mut Frame, area: Rect) {
        if let Some(order) = self.get_selected_order() {
            let total: Decimal = order
                .trades
                .iter()
                .map(|trade| trade.amount * trade.price)
                .sum();
            let block = Block::default()
                .title(format!("{} trades : $ {}", order.trades.len(), total))
                .borders(Borders::ALL);

            let mut assets: Vec<&String> = self.portfolio.assets.keys().collect();
            assets.sort();

            let items: Vec<ListItem> = order
                .trades
                .iter()
                .map(|trade| ListItem::from(trade))
                .collect();
            let list = List::new(items).block(block);
            frame.render_widget(list, area);
        }
    }

    fn render_trades(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default().title("Trades").borders(Borders::ALL);

        let mut error: Option<String> = None;
        match self
            .selected_asset
            .clone()
            .and_then(|k| self.portfolio.assets.get(&k))
        {
            Some(asset) => {
                let block = Block::default()
                    .title(format!("{} trades", asset.symbol))
                    .borders(Borders::ALL);
                let ticker = Ticker::new(asset.symbol.as_str(), &self.quote);
                if let Some(trades) = self.trades.get(&ticker) {
                    let data: Vec<Option<u64>> = trades
                        .iter()
                        .enumerate()
                        .map(|(i, trade)| {
                            //(i as f64, i as f64)
                            //(trade.trade_time as f64, trade.price.to_f64().unwrap())
                            (trade.quantity * trade.price).to_u64()
                        })
                        .collect();

                    if data.is_empty() {
                        error = Some(format!("No trades for {} yet", ticker));
                    } else {
                        //let min_d = data.iter().map(|d| d.0).reduce(f64::min).unwrap_or(0.);
                        //let max_d = data.iter().map(|d| d.0).reduce(f64::max).unwrap_or(0.);
                        //
                        //let min_close = data.iter().map(|d| d.1).reduce(f64::min).unwrap_or(0.);
                        //let max_close = data.iter().map(|d| d.1).reduce(f64::max).unwrap_or(0.);
                        //
                        let chart = Sparkline::default()
                            .data(&data)
                            .block(block.clone())
                            .style(Style::default().fg(tailwind::SLATE.c900));

                        frame.render_widget(chart, area);
                    }
                } else {
                    error = Some(format!("No trades for {} yet", ticker));
                }
            }
            None => {
                error = Some(format!("Select an asset"));
            }
        }

        if let Some(error) = error {
            let p = Paragraph::new(Line::from(error)).block(block);
            frame.render_widget(p, area);
        }
    }

    fn render_candles(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::default().title("Candles").borders(Borders::ALL);

        let mut error: Option<String> = None;
        match self
            .selected_asset
            .clone()
            .and_then(|k| self.portfolio.assets.get(&k))
        {
            Some(asset) => {
                let block = Block::default()
                    .title(format!("{} candles", asset.symbol))
                    .borders(Borders::ALL);
                let ticker = Ticker::new(asset.symbol.as_str(), &self.quote);
                if let Some(candles) = self.candles.get_mut(&ticker) {
                    let data_candles: Vec<(f64, f64)> = candles
                        .make_contiguous()
                        .windows(10)
                        .map(|candles| {
                            (
                                candles
                                    .iter()
                                    .map(|candle| candle.close_time)
                                    .max()
                                    .unwrap() as f64,
                                avg(&candles
                                    .iter()
                                    .map(|candle| candle.close_price)
                                    .collect::<Vec<Decimal>>())
                                .unwrap()
                                .to_f64()
                                .unwrap(),
                            )
                        })
                        .collect();
                    if data_candles.is_empty() {
                        error = Some(format!("No candles for {} yet", ticker));
                    } else {
                        let start = data_candles
                            .iter()
                            .map(|d| d.0)
                            .reduce(f64::min)
                            .unwrap_or(0.);
                        let end = data_candles
                            .iter()
                            .map(|d| d.0)
                            .reduce(f64::max)
                            .unwrap_or(0.);

                        let min_close = data_candles
                            .iter()
                            .map(|d| d.1)
                            .reduce(f64::min)
                            .unwrap_or(0.);
                        let max_close = data_candles
                            .iter()
                            .map(|d| d.1)
                            .reduce(f64::max)
                            .unwrap_or(0.);

                        let last_close = if data_candles.len() > 0 {
                            data_candles[data_candles.len() - 1].1
                        } else {
                            0.
                        };

                        let start_time = if data_candles.len() > 0 {
                            DateTime::from_timestamp_millis(data_candles[0].0 as i64)
                                .unwrap()
                                .format("%d %H:%M")
                                .to_string()
                        } else {
                            String::from("-")
                        };

                        let end_time = if data_candles.len() > 0 {
                            DateTime::from_timestamp_millis(
                                data_candles[data_candles.len() - 1].0 as i64,
                            )
                            .unwrap()
                            .format("%d %H:%M")
                            .to_string()
                        } else {
                            String::from("-")
                        };

                        let dataset_candles = Dataset::default()
                            .data(&data_candles)
                            .marker(symbols::Marker::Braille)
                            .style(Style::default().fg(tailwind::WHITE))
                            .graph_type(ratatui::widgets::GraphType::Line);

                        let data_buy_orders: Vec<(f64, f64)> = self
                            .get_orders()
                            .iter()
                            .filter(|order| order.side == OrderSide::Buy)
                            .map(|order| {
                                (order.creation_time as f64, order.price.to_f64().unwrap())
                            })
                            .collect();
                        let dataset_buy_orders = Dataset::default()
                            .data(&data_buy_orders)
                            .marker(symbols::Marker::Dot)
                            .style(Style::default().fg(tailwind::GREEN.c500))
                            .graph_type(ratatui::widgets::GraphType::Scatter);

                        let data_sell_orders: Vec<(f64, f64)> = self
                            .get_orders()
                            .iter()
                            .filter(|order| order.side == OrderSide::Sell)
                            .map(|order| {
                                (order.creation_time as f64, order.price.to_f64().unwrap())
                            })
                            .collect();
                        let dataset_sell_orders = Dataset::default()
                            .data(&data_sell_orders)
                            .marker(symbols::Marker::Dot)
                            .style(Style::default().fg(tailwind::RED.c500))
                            .graph_type(ratatui::widgets::GraphType::Scatter);

                        let chart = Chart::new(vec![
                            dataset_candles,
                            dataset_buy_orders,
                            dataset_sell_orders,
                        ])
                        .x_axis(
                            Axis::default()
                                .title("Time")
                                .bounds([start, end])
                                .labels([start_time, end_time]),
                        )
                        .y_axis(
                            Axis::default()
                                .title("Price")
                                .bounds([min_close, max_close])
                                .labels([
                                    format!("{}", min_close),
                                    format!("{}", last_close),
                                    format!("{}", max_close),
                                ]),
                        )
                        .block(block.clone());

                        frame.render_widget(chart, area);
                    }
                } else {
                    error = Some(format!("No candles for {} yet", ticker));
                }
            }
            None => {
                error = Some(format!("Select an asset"));
            }
        }

        if let Some(error) = error {
            let p = Paragraph::new(Line::from(error)).block(block);
            frame.render_widget(p, area);
        }
    }

    fn render_strategy_log(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default().title("Strategy").borders(Borders::ALL);
        let mut error: Option<String> = None;
        match self
            .selected_asset
            .clone()
            .and_then(|k| self.portfolio.assets.get(&k))
        {
            Some(asset) => {
                let ticker = Ticker::new(asset.symbol.as_str(), &self.quote);
                match self.last_strategy_events.get(&ticker) {
                    Some(events) => {
                        let block = Block::default()
                            .title(format!("{} strategy", ticker.base))
                            .borders(Borders::ALL);
                        let mut items: Vec<ListItem> = vec![];
                        for (reason, (count, details)) in events.iter() {
                            items.push(ListItem::from(vec![
                                Line::from(format!("{} ({})", reason, count)),
                                Line::from(format!("{}", details)),
                            ]));
                        }
                        let list = List::new(items).block(block);
                        frame.render_widget(list, area);
                    }
                    None => {
                        error = Some(format!("No events yet"));
                    }
                }
            }
            None => {
                error = Some(format!("Select an asset"));
            }
        }
        if let Some(error) = error {
            let p = Paragraph::new(Line::from(error)).block(block);
            frame.render_widget(p, area);
        }
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title("Trading Bot Monitoring")
            .borders(Borders::ALL);
        frame.render_widget(block, area);
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default().borders(Borders::ALL);
        let keys = match self.selected_window {
            Window::Portfolio => {
                "(Esc) back | (p) Pause/Resume | (1) Portfolio | (2) Orders | (↑) previous asset | (↓) next asset"
            }
            Window::Orders => {
                "(Esc) back | (p) Pause/Resume | (1) Portfolio | (2) Orders | (↑) previous order | (↓) next order"
            }
            Window::None => "(Esc) quit | (p) Pause/Resume | (1) Portfolio | (2) Orders",
        };

        let keys = Paragraph::new(Text::from(keys)).block(block);
        frame.render_widget(keys, area);
    }
}

impl From<&Asset> for ListItem<'_> {
    fn from(asset: &Asset) -> Self {
        let sl = asset.symbol.len();
        let top_line = Line::from(vec![
            Span::styled(asset.symbol.clone(), Style::new().fg(Color::Blue)),
            Span::raw(" ".repeat(if sl < 4 { sl - 1 } else { 1 })),
            Span::styled(
                format!(
                    "$ {}",
                    asset
                        .value
                        .map_or("?".to_string(), |value| value.round_dp(2).to_string())
                ),
                Style::default().fg(Color::Yellow),
            ),
        ]);
        let bot_line = Line::from(vec![Span::raw(asset.amount.to_string())]);
        ListItem::new(vec![top_line, bot_line])
    }
}

impl From<&OrderTrade> for ListItem<'_> {
    fn from(value: &OrderTrade) -> Self {
        let top_line = Line::from(Span::styled(
            format!("$ {}", value.amount * value.price),
            Style::new().fg(Color::Yellow),
        ));
        let bot_line = Line::from(vec![
            Span::raw(format!("{}", value.amount)),
            Span::raw(" - "),
            Span::raw(format!("{}", value.price)),
        ]);
        ListItem::new(vec![top_line, bot_line])
    }
}
