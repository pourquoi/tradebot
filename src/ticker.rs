use serde::{Deserialize, Serialize};
use std::fmt::Display;

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
pub struct Ticker {
    #[serde(rename = "b")]
    pub base: String,
    #[serde(rename = "q")]
    pub quote: String,
}

impl Ticker {
    pub fn new(base: &str, quote: &str) -> Self {
        Self {
            base: base.to_string(),
            quote: quote.to_string(),
        }
    }
}

impl TryFrom<&str> for Ticker {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.len() {
            6 => Ok(Self {
                base: value[0..=2].to_string(),
                quote: value[3..=5].to_string(),
            }),
            7 => Ok(Self {
                base: value[0..=2].to_string(),
                quote: value[3..=6].to_string(),
            }),
            _ => Err(()),
        }
    }
}

impl Display for Ticker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.base, self.quote)
    }
}
