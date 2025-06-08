use rust_decimal::Decimal;

pub fn ceil_to_step(value: Decimal, step: Decimal) -> Decimal {
    ((value / step).ceil()) * step
}
