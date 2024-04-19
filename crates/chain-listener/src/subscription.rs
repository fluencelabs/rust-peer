use crate::event::cc_activated::CommitmentActivated;
use crate::event::{ComputeUnitMatched, UnitActivated, UnitDeactivated};
use alloy_sol_types::SolEvent;
use backoff::future::retry;
use backoff::Error::Permanent;
use backoff::ExponentialBackoff;
use chain_connector::CommitmentId;
use chain_data::peer_id_to_hex;
use jsonrpsee::core::client::{Error, Subscription, SubscriptionClientT};
use jsonrpsee::core::params::ArrayParams;
use jsonrpsee::core::{async_trait, JsonValue};
use jsonrpsee::rpc_params;
use jsonrpsee::ws_client::{WsClient, WsClientBuilder};
use libp2p_identity::PeerId;
use serde_json::json;

#[async_trait]
pub trait EventSubscription: Send + Sync {
    async fn unit_activated(
        &self,
        commitment_id: &CommitmentId,
    ) -> Result<Subscription<JsonValue>, Error>;
    async fn unit_deactivated(
        &self,
        commitment_id: &CommitmentId,
    ) -> Result<Subscription<JsonValue>, Error>;
    async fn new_heads(&self) -> Result<Subscription<JsonValue>, Error>;
    async fn commitment_activated(&self) -> Result<Subscription<JsonValue>, Error>;
    async fn unit_matched(&self) -> Result<Subscription<JsonValue>, Error>;


    // TODO: remove both methods and encapsulate logic
    async fn refresh(&mut self) -> Result<(), Error>;
    async fn restart(&mut self) -> Result<(), Error>;
}

pub struct WsSubscriptionConfig {
    host_id: PeerId,
    ws_endpoint: String,
    cc_contract_address: String,
    market_contract_address: String,
}

impl WsSubscriptionConfig {
    pub fn new(
        host_id: PeerId,
        ws_endpoint: String,
        cc_contract_address: String,
        market_contract_address: String,
    ) -> Self {
        Self {
            host_id,
            ws_endpoint,
            cc_contract_address,
            market_contract_address,
        }
    }
}

pub struct WsEventSubscription {
    config: WsSubscriptionConfig,
    ws_client: WsClient,
}

impl WsEventSubscription {
    pub async fn new(config: WsSubscriptionConfig) -> Result<Self, Error> {
        let ws_client = Self::create_ws_client(config.ws_endpoint.as_str()).await?;
        Ok(Self { config, ws_client })
    }

    async fn subscribe(
        &self,
        method: &str,
        params: ArrayParams,
    ) -> Result<Subscription<JsonValue>, Error> {
        let sub = retry(ExponentialBackoff::default(), || async {
           self.ws_client
                .subscribe("eth_subscribe", params.clone(), "eth_unsubscribe")
                .await.map_err(|err|  {
                if let Error::RestartNeeded(_) = err {
                    tracing::error!(target: "chain-listener", "Failed to subscribe to {method}: {err};");
                    Permanent(err)
                } else {
                    tracing::warn!(target: "chain-listener", "Failed to subscribe to {method}: {err}; Retrying...");
                    backoff::Error::transient(err)
                }})
        }).await?;

        Ok(sub)
    }

    fn unit_activated_params(&self, commitment_id: &CommitmentId) -> ArrayParams {
        let topic = UnitActivated::SIGNATURE_HASH.to_string();
        rpc_params![
            "logs",
            json!({"address": self.config.cc_contract_address, "topics":  vec![topic, hex::encode(commitment_id.0)]})
        ]
    }

    fn unit_deactivated_params(&self, commitment_id: &CommitmentId) -> ArrayParams {
        let topic = UnitDeactivated::SIGNATURE_HASH.to_string();
        rpc_params![
            "logs",
            json!({"address": self.config.cc_contract_address, "topics":  vec![topic, hex::encode(commitment_id.0)]})
        ]
    }

    fn cc_activated_params(&self) -> ArrayParams {
        let topic = CommitmentActivated::SIGNATURE_HASH.to_string();
        let topics = vec![topic, peer_id_to_hex(self.config.host_id)];
        rpc_params![
            "logs",
            json!({"address": self.config.cc_contract_address, "topics": topics})
        ]
    }

    fn unit_matched_params(&self) -> ArrayParams {
        let topics = vec![
            ComputeUnitMatched::SIGNATURE_HASH.to_string(),
            peer_id_to_hex(self.config.host_id),
        ];
        rpc_params![
            "logs",
            json!({"address": self.config.market_contract_address, "topics": topics})
        ]
    }

    async fn create_ws_client(ws_endpoint: &str) -> Result<WsClient, Error> {
        let ws_client = retry(ExponentialBackoff::default(), || async {
            let client = WsClientBuilder::default()
                .build(ws_endpoint)
                .await
                .map_err(|err| {
                    tracing::warn!(
                        target: "chain-listener",
                        "Error connecting to websocket endpoint {}, error: {}; Retrying...",
                        ws_endpoint,
                        err
                    );
                    err
                })?;

            Ok(client)
        })
        .await?;

        tracing::info!(
            target: "chain-listener",
            "Successfully connected to websocket endpoint: {}",
            ws_endpoint
        );

        Ok(ws_client)
    }
}

#[async_trait]
impl EventSubscription for WsEventSubscription {
    async fn unit_activated(
        &self,
        commitment_id: &CommitmentId,
    ) -> Result<Subscription<JsonValue>, Error> {
        self.subscribe("logs", self.unit_activated_params(commitment_id))
            .await
    }

    async fn unit_deactivated(
        &self,
        commitment_id: &CommitmentId,
    ) -> Result<Subscription<JsonValue>, Error> {
        self.subscribe("logs", self.unit_deactivated_params(commitment_id))
            .await
    }

    async fn new_heads(&self) -> Result<Subscription<JsonValue>, Error> {
        self.subscribe("newHeads", rpc_params!["newHeads"]).await
    }

    async fn commitment_activated(&self) -> Result<Subscription<JsonValue>, Error> {
        self.subscribe("logs", self.cc_activated_params()).await
    }

    async fn unit_matched(&self) -> Result<Subscription<JsonValue>, Error> {
        self.subscribe("logs", self.unit_matched_params()).await
    }

    async fn refresh(&mut self) -> Result<(), Error> {
        if !self.ws_client.is_connected() {
            self.restart().await?
        }
        Ok(())
    }

    async fn restart(&mut self) -> Result<(), Error> {
        self.ws_client = Self::create_ws_client(&self.config.ws_endpoint).await?;
        Ok(())
    }
}
