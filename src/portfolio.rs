use anyhow::{anyhow, Result};
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fmt::Display};

use colored::Colorize;
use rust_decimal::Decimal;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Asset {
    pub symbol: String,
    pub amount: Decimal,
    pub locked: Decimal,
    pub value: Option<Decimal>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Portfolio {
    pub assets: HashMap<String, Asset>,
    pub value: Option<Decimal>,
}

impl Default for Portfolio {
    fn default() -> Self {
        Self::new()
    }
}

impl Portfolio {
    pub fn new() -> Self {
        Self {
            assets: HashMap::new(),
            value: None,
        }
    }

    pub fn update_asset_value(&mut self, symbol: &str, price: Decimal) {
        if let Some(asset) = self.assets.get_mut(symbol) {
            asset.value = Some((asset.locked + asset.amount) * price);
            self.update_value();
        }
    }

    pub fn check_funds(&self, asset: &str, required_amount: Decimal) -> bool {
        match self.assets.get(asset) {
            Some(asset) => asset.amount >= required_amount,
            None => false,
        }
    }

    pub fn reserve_funds(&mut self, asset: &String, amount: Decimal) -> Result<()> {
        match self.assets.get_mut(asset) {
            Some(asset) => {
                if asset.amount < amount {
                    return Err(anyhow!(
                        "Missing {} for {}",
                        amount - asset.amount,
                        asset.symbol
                    ));
                }
                asset.amount -= amount;
                asset.locked += amount;
                Ok(())
            }
            None => Err(anyhow!("Asset {} not in portfolio", asset)),
        }
    }

    // for stable iteration over assets
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

    pub fn update_asset(&mut self, asset: Asset) {
        self.assets.insert(asset.symbol.clone(), asset);
    }

    pub fn update_asset_amount(&mut self, symbol: &str, delta: Decimal, current_price: Decimal) {
        self.assets
            .entry(symbol.to_owned())
            .and_modify(|asset| {
                asset.amount += delta;
                asset.value = Some((asset.amount + asset.locked) * current_price);
            })
            .or_insert(Asset {
                symbol: symbol.to_owned(),
                amount: delta,
                locked: dec!(0),
                value: Some(delta * current_price),
            });

        self.update_value();
    }

    pub fn drain_asset_locked(&mut self, symbol: &str, drain: Decimal, current_price: Decimal) {
        self.assets
            .entry(symbol.to_owned())
            .and_modify(|asset| {
                asset.locked -= drain;
                asset.value = Some((asset.amount + asset.locked) * current_price);
            })
            .or_insert(Asset {
                symbol: symbol.to_owned(),
                amount: -drain,
                locked: dec!(0),
                value: Some(-drain * current_price),
            });

        self.update_value();
    }

    pub fn update_value(&mut self) {
        self.value = Some(
            self.assets
                .iter()
                .flat_map(|(_, asset)| asset.value)
                .sum(),
        );
    }
}

impl Display for Portfolio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = vec![];
        for (_, asset) in self.assets.iter() {
            s.push(format!(
                "{}: amount={} locked={} total=${} unit price=${})",
                asset.symbol.blue(),
                asset.amount.to_string().purple(),
                asset.locked.to_string().purple(),
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
            "\n---\n${}\n{}\n---",
            self.value
                .map_or("?".to_string(), |v| v.to_string())
                .yellow(),
            s.join("\n")
        )
    }
}
