use crate::{
    marketplace::{MarketplaceAccountStream, MarketplaceSettingsApi},
    AppEvent,
};

use super::SimulationMarketplace;

impl<S: MarketplaceSettingsApi> MarketplaceAccountStream for SimulationMarketplace<S> {
    async fn start_account_stream(
        &mut self,
        tx_app: tokio::sync::broadcast::Sender<crate::AppEvent>,
    ) -> anyhow::Result<()> {
        let mut rx = self.tx_account.subscribe();
        loop {
            if let Ok(event) = rx.recv().await {
                let _ = tx_app.send(AppEvent::MarketPlace(event));
            }
        }
    }
}
