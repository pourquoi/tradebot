use rust_decimal::prelude::*;
use rust_decimal_macros::dec;
use std::{collections::HashMap, usize};
use yata::{
    core::{Method, PeriodType},
    methods::WSMA,
    prelude::Peekable,
};

use rust_decimal::Decimal;

pub fn sma(prices: &[Decimal], period: usize) -> Option<Decimal> {
    if prices.len() >= period {
        Some(prices[0..period].iter().sum::<Decimal>() / Decimal::from(period as u32))
    } else {
        None
    }
}

pub fn wsma(prices: &[Decimal], period: usize) -> Option<Decimal> {
    if period > 0 && prices.len() >= period {
        let first = prices[period - 1].to_f64()?;
        let mut w = WSMA::new(period as PeriodType, &first).ok()?;
        for p in prices[1..period].iter().rev() {
            let f = p.to_f64().unwrap();
            w.next(&f);
        }
        Decimal::from_f64_retain(w.peek())
    } else {
        None
    }
}

pub fn atr(prices: &[(Decimal, Decimal, Option<Decimal>)], n: usize) -> Option<Decimal> {
    if prices.len() < n {
        return None;
    }

    let mut tr_sum = dec!(0);
    let mut count = 0;

    for &(high, low, prev_close) in &prices[0..n] {
        let tr = if let Some(prev_close) = prev_close {
            [
                high - low,
                (high - prev_close).abs(),
                (low - prev_close).abs(),
            ]
            .iter()
            .copied()
            .max()
            .unwrap()
        } else {
            high - low
        };

        tr_sum += tr;
        count += 1;
    }

    if count > 0 {
        Some(tr_sum / Decimal::from(count))
    } else {
        None
    }
}

pub fn avg(prices: &[Decimal]) -> Option<Decimal> {
    let iter = prices.iter();
    let (sum, count) = iter.fold((Decimal::ZERO, 0), |(s, c), p| (s + p, c + 1));

    if count == 0 {
        None
    } else {
        Some(sum / Decimal::from(count))
    }
}

pub fn percentiles(prices: &Vec<Decimal>) -> HashMap<u8, Decimal> {
    let mut prices = prices.clone();
    prices.sort_unstable();
    let mut percentiles: HashMap<u8, Decimal> = HashMap::new();
    let len = prices.len() as f64;
    for p in (10..=90).step_by(10) {
        let n = (len * (p as f64 / 100_f64)).round();
        let n = n.min(len - 1.);
        percentiles.insert(p as u8, prices[n as usize]);
    }
    percentiles
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_candles_percentiles() {
        let prices = vec![dec!(0)];
        let p = percentiles(&prices);
        assert_eq!(
            p,
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

        let prices = vec![dec!(0.5), dec!(1.5), dec!(1.5), dec!(10)];
        let p = percentiles(&prices);
        assert_eq!(
            p,
            HashMap::from([
                (10_u8, dec!(0.5)),
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

    #[test]
    fn test_sma() {
        let v = vec![dec!(1)];
        assert_eq!(Some(dec!(1)), sma(&v, 1));

        let v = vec![dec!(1), dec!(2), dec!(3)];
        assert_eq!(dec!(2), sma(&v, 3).unwrap().round_dp(1));

        let v = vec![dec!(1), dec!(2), dec!(3), dec!(4)];
        assert_eq!(dec!(2), sma(&v, 3).unwrap().round_dp(1));
    }

    #[test]
    fn test_wsma() {
        let v = vec![dec!(1)];
        assert_eq!(Some(dec!(1)), wsma(&v, 1));

        let v = vec![dec!(1), dec!(2), dec!(3)];
        assert_eq!(dec!(2.7), wsma(&v, 3).unwrap().round_dp(1));

        let v = vec![dec!(1), dec!(2), dec!(3), dec!(4)];
        assert_eq!(dec!(2.7), wsma(&v, 3).unwrap().round_dp(1));
    }
}
