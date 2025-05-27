use std::{collections::HashMap, usize};

use rust_decimal::Decimal;

pub fn short_term_sma(prices: &[Decimal], period: usize) -> Option<Decimal> {
    if prices.len() >= period {
        Some(prices[prices.len() - period..].iter().sum::<Decimal>() / Decimal::from(period as u32))
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
}
