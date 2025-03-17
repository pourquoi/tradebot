use std::{collections::HashMap, usize};

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::binance::Candle;

pub fn dec_avg<T>(prices: T) -> Decimal
where
    T: IntoIterator<Item = Decimal>,
{
    let iter = prices.into_iter();
    let (sum, count) = iter.fold((Decimal::ZERO, 0), |(s, c), p| (s + p, c + 1));

    if count == 0 {
        Decimal::ZERO
    } else {
        sum / Decimal::from(count)
    }
}

pub fn candles_avg(candles: &Vec<Candle>) -> Decimal {
    let prices = candles
        .iter()
        .map(|c| (c.low_price + c.high_price) / dec!(2))
        .collect::<Vec<Decimal>>();
    let avg = dec_avg(prices);
    avg
}

pub fn candles_percentiles(candles: &Vec<Candle>) -> HashMap<u8, Decimal> {
    let mut candles = candles.clone();
    candles.sort_by_cached_key(|candle| (candle.high_price + candle.low_price) / dec!(2));
    let mut percentiles: HashMap<u8, Decimal> = HashMap::new();
    let len = candles.len() as f64;
    for p in (10..=90).step_by(10) {
        let n = (len * (p as f64 / 100_f64)).round() as usize;
        let n = n.min(candles.len() - 1);
        match candles.get(n) {
            Some(candle) => {
                percentiles.insert(p as u8, (candle.low_price + candle.high_price) / dec!(2))
            }
            None => percentiles.insert(p, dec!(0)),
        };
    }
    percentiles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_candles_percentiles() {
        let candles = vec![Candle {
            high_price: dec!(0),
            low_price: dec!(0),
            ..Default::default()
        }];
        let percentiles = candles_percentiles(&candles);
        assert_eq!(
            percentiles,
            HashMap::from([
                (10_u8, dec!(0)),
                (20_u8, dec!(0)),
                (30_u8, dec!(0)),
                (40_u8, dec!(0)),
                (50_u8, dec!(0)),
                (60_u8, dec!(0)),
                (70_u8, dec!(0)),
                (80_u8, dec!(0)),
                (90_u8, dec!(0)),
            ])
        );

        let candles = vec![
            Candle {
                high_price: dec!(1),
                low_price: dec!(2),
                ..Default::default()
            },
            Candle {
                high_price: dec!(1),
                low_price: dec!(2),
                ..Default::default()
            },
            Candle {
                high_price: dec!(1),
                low_price: dec!(2),
                ..Default::default()
            },
            Candle {
                high_price: dec!(10),
                low_price: dec!(10),
                ..Default::default()
            },
        ];
        let percentiles = candles_percentiles(&candles);
        assert_eq!(
            percentiles,
            HashMap::from([
                (10_u8, dec!(1.5)),
                (20_u8, dec!(1.5)),
                (30_u8, dec!(1.5)),
                (40_u8, dec!(1.5)),
                (50_u8, dec!(1.5)),
                (60_u8, dec!(1.5)),
                (70_u8, dec!(10)),
                (80_u8, dec!(10)),
                (90_u8, dec!(10)),
            ])
        );
    }
}
