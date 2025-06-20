use std::{collections::HashMap, time::Duration};

use crate::{marketplace::MarketplaceAccountApi, portfolio::Asset};
use crate::marketplace::{MarketplaceSettingsApi};
use super::{SimulationMarketplace};

impl<S: MarketplaceSettingsApi> MarketplaceAccountApi for SimulationMarketplace<S> {
    async fn get_account_assets(&mut self) -> anyhow::Result<HashMap<String, Asset>> {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let assets = self.assets.read().await;
        Ok(assets.clone())
    }
}
