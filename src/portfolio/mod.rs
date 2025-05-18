use std::collections::HashMap;

use rust_decimal::Decimal;

#[derive(Clone, Debug)]
pub struct Portfolio {
    pub assets: HashMap<String, Asset>,
}

#[derive(Clone, Debug)]
pub struct Asset {
    pub ticker: String,
    pub amount: Decimal,
}
