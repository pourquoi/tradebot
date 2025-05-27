use rust_decimal_macros::dec;
use std::{collections::HashMap, fmt::Display};

use colored::Colorize;
use rust_decimal::Decimal;

#[derive(Clone, Debug)]
pub struct Asset {
    pub symbol: String,
    pub amount: Decimal,
    pub value: Option<Decimal>,
}

#[derive(Clone, Debug)]
pub struct Portfolio {
    pub assets: HashMap<String, Asset>,
}

impl Portfolio {
    pub fn new() -> Self {
        Self {
            assets: HashMap::new(),
        }
    }
}

impl Display for Portfolio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = vec![];
        let mut total = dec!(0);
        for (_, asset) in self.assets.iter() {
            s.push(format!(
                "{}: {} (~{})",
                asset.symbol,
                asset.amount.to_string().purple(),
                asset.value.unwrap_or(dec!(0))
            ));
            total += asset.value.unwrap_or(dec!(0));
        }
        write!(f, "~{} : {}", total.to_string().yellow(), s.join(" / "))
    }
}
