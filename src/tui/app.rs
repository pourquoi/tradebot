use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode};
use futures_util::stream::StreamExt as _;
use futures_util::StreamExt;
use ratatui::{
    layout::{
        Constraint::{self},
        Layout, Rect,
    },
    style::{Color, Style},
    symbols,
    text::{Line, Span},
    widgets::{Axis, Block, Borders, Chart, Dataset, List, ListItem, Paragraph, Widget},
    Frame,
};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal_macros::dec;
use std::{
    collections::{HashMap, VecDeque},
    time::Duration,
};
use tokio::sync::mpsc::Receiver;

use crate::{
    marketplace::{CandleEvent, MarketPlaceEvent},
    portfolio::{Asset, Portfolio, PortfolioEvent},
    strategy::StrategyEvent,
    ticker::Ticker,
    AppEvent,
};

enum Window {
    Portfolio,
}

pub struct App {
    should_quit: bool,
    rx: Receiver<AppEvent>,
    portfolio: Portfolio,
    last_strategy_events: HashMap<Ticker, String>,
    candles: HashMap<Ticker, VecDeque<CandleEvent>>,
    selected_window: Window,
    selected_asset: Option<String>,
}

impl App {
    pub fn new(rx: Receiver<AppEvent>) -> Self {
        Self {
            should_quit: false,
            rx,
            last_strategy_events: HashMap::new(),
            candles: HashMap::new(),
            portfolio: Portfolio::new(),
            selected_asset: None,
            selected_window: Window::Portfolio,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        let mut terminal = ratatui::init();
        let _ = terminal.clear();

        let mut events = EventStream::new();

        let period = Duration::from_secs_f64(1.0 / 20.0);
        let mut interval = tokio::time::interval(period);

        while !self.should_quit {
            tokio::select! {
                _ = interval.tick() => { terminal.draw(|frame| self.render(frame))?; },
                Some(Ok(event)) = events.next() => self.handle_events(event),
                Some(event) = self.rx.recv() =>
                    self.handle_app_events(event)
            }
        }

        Ok(())
    }

    fn handle_app_events(&mut self, event: AppEvent) {
        match event {
            AppEvent::Portfolio(PortfolioEvent::Status(portfolio)) => {
                self.portfolio = portfolio.clone();
            }
            AppEvent::Strategy(StrategyEvent::Info { message, order }) => {
                if let Some(order) = order {
                    self.last_strategy_events
                        .insert(order.ticker.clone(), message);
                }
            }
            AppEvent::MarketPlace(MarketPlaceEvent::Candle(candle)) => {
                let candles = self
                    .candles
                    .entry(candle.ticker.clone())
                    .or_insert(VecDeque::with_capacity(300));
                if let Some(last) = candles.pop_back() {
                    if last.start_time != candle.start_time {
                        candles.push_back(last);
                    }
                }
                candles.push_back(candle.clone());
                if candles.len() >= 100 {
                    candles.pop_front();
                }
            }
            _ => {}
        }
    }

    fn handle_events(&mut self, event: Event) {
        if let Some(key) = event.as_key_press_event() {
            match key.code {
                KeyCode::Char('q') => self.should_quit = true,
                KeyCode::Up => match self.selected_window {
                    Window::Portfolio => {
                        self.selected_asset = self
                            .portfolio
                            .next_prev_symbol(self.selected_asset.clone(), false);
                    }
                },
                KeyCode::Down => match self.selected_window {
                    Window::Portfolio => {
                        self.selected_asset = self
                            .portfolio
                            .next_prev_symbol(self.selected_asset.clone(), true);
                    }
                },
                _ => {}
            }
        }
    }

    fn render(&self, frame: &mut Frame) {
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

        self.render_header(frame, header_area);
        self.render_footer(frame, footer_area);
        self.render_portfolio(frame, top_left_area);
        self.render_chart(frame, right_area);
    }

    fn render_portfolio(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default().title("Portfolio").borders(Borders::ALL);
        let mut assets: Vec<&String> = self.portfolio.assets.keys().collect();
        assets.sort();

        let items: Vec<ListItem> = assets
            .iter()
            .flat_map(|symbol| {
                self.portfolio
                    .assets
                    .get(*symbol)
                    .map(|asset| ListItem::from(asset))
            })
            .collect();
        let list = List::new(items).block(block);
        frame.render_widget(list, area);
    }

    fn render_chart(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default().title("Graph").borders(Borders::ALL);

        let mut error: Option<String> = None;
        match self
            .selected_asset
            .clone()
            .and_then(|k| self.portfolio.assets.get(&k))
        {
            Some(asset) => {
                let block = Block::default()
                    .title(format!("{}", asset.symbol))
                    .borders(Borders::ALL);
                let ticker = Ticker::new(asset.symbol.as_str(), "USDT");
                if let Some(candles) = self.candles.get(&ticker) {
                    let data: Vec<(f64, f64)> = candles
                        .iter()
                        .enumerate()
                        .map(|(i, candle)| {
                            //(i as f64, i as f64)
                            (
                                candle.start_time as f64,
                                candle.close_price.unwrap_or(dec!(0)).to_f64().unwrap(),
                            )
                        })
                        .collect();
                    if data.is_empty() {
                        error = Some(format!("No candles for {} yet", ticker));
                    } else {
                        let start = data.iter().map(|d| d.0).reduce(f64::min).unwrap_or(0.);
                        let end = data.iter().map(|d| d.0).reduce(f64::max).unwrap_or(0.);

                        let min_close = data.iter().map(|d| d.1).reduce(f64::min).unwrap_or(0.);
                        let max_close = data.iter().map(|d| d.1).reduce(f64::max).unwrap_or(0.);

                        let dataset = Dataset::default()
                            .data(&data)
                            .marker(symbols::Marker::Dot)
                            .style(Style::default().fg(Color::Blue))
                            .graph_type(ratatui::widgets::GraphType::Line);

                        let chart = Chart::new(vec![dataset])
                            .x_axis(Axis::default().title("Time").bounds([start, end]))
                            .y_axis(
                                Axis::default()
                                    .title("Price")
                                    .bounds([min_close, max_close]),
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

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title("Trading Bot Monitoring")
            .borders(Borders::ALL);
        frame.render_widget(block, area);
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default().borders(Borders::ALL);
        let p1 = Paragraph::new(Line::from("Press 'q' to quit")).block(block);
        frame.render_widget(p1, area);
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
