use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fmt::Display};

use colored::Colorize;
use rust_decimal::Decimal;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Asset {
    pub symbol: String,
    pub amount: Decimal,
    pub value: Option<Decimal>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Portfolio {
    pub assets: HashMap<String, Asset>,
    pub value: Option<Decimal>,
}

impl Portfolio {
    pub fn new() -> Self {
        Self {
            assets: HashMap::new(),
            value: None,
        }
    }

    pub fn next_prev_symbol(&self, cur: Option<String>, is_next: bool) -> Option<String> {
        let mut keys: Vec<&String> = self.assets.keys().collect();
        keys.sort();

        if keys.is_empty() {
            return None;
        }

        match cur {
            None => Some(keys[0].clone()),
            Some(cur) => match keys.iter().position(|k| *k == &cur) {
                Some(pos) => Some(
                    keys[if is_next {
                        pos + 1
                    } else {
                        pos + keys.len() - 1
                    } % keys.len()]
                    .clone(),
                ),
                None => Some(keys[0].clone()),
            },
        }
    }

    pub fn update_asset_amount(&mut self, symbol: &String, delta: Decimal, current_price: Decimal) {
        self.assets
            .entry(symbol.clone())
            .and_modify(|asset| {
                asset.amount += delta;
                asset.value = Some(asset.amount * current_price);
            })
            .or_insert(Asset {
                symbol: symbol.clone(),
                amount: delta,
                value: Some(delta * current_price),
            });

        self.update_value();
    }

    pub fn update_value(&mut self) {
        self.value = Some(
            self.assets
                .iter()
                .flat_map(|(_, asset)| asset.value.map(|value| value))
                .sum(),
        );
    }
}

impl Display for Portfolio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = vec![];
        for (_, asset) in self.assets.iter() {
            s.push(format!(
                "{}: {} (~total {}, unit {})",
                asset.symbol,
                asset.amount.to_string().purple(),
                asset.value.unwrap_or(dec!(0)),
                if asset.amount != dec!(0) {
                    asset.value.unwrap_or(dec!(0)) / asset.amount
                } else {
                    dec!(0)
                }
            ));
        }
        write!(
            f,
            "~{} :\n{}",
            self.value
                .map_or("?".to_string(), |v| v.to_string())
                .yellow(),
            s.join("\n")
        )
    }
}
