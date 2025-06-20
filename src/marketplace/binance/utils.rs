use rust_decimal::Decimal;

pub fn ceil_to_step(value: Decimal, step: Decimal) -> Decimal {
    ((value / step).ceil()) * step
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_ceil_to_step() {
        let value = dec!(300.001);
        let step = dec!(0.01);
        assert_eq!(dec!(300.01), ceil_to_step(value, step));

        let value = dec!(10.489630);
        let step = dec!(0.01);
        assert_eq!(dec!(10.49), ceil_to_step(value, step));
    }
}
