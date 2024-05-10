/*
 * Copyright 2020 Fluence Labs Limited
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */
use libp2p::identify::Config as IdentifyConfig;
use libp2p::{
    connection_limits::Behaviour as ConnectionLimits,
    identify::Behaviour as Identify,
    ping::{Behaviour as Ping, Config as PingConfig},
    swarm::NetworkBehaviour,
    PeerId,
};
use libp2p_swarm::StreamProtocol;
use thiserror::Error;
use tokio::sync::mpsc;

use connection_pool::ConnectionPoolBehaviour;
use health::HealthCheckRegistry;
use kademlia::{Kademlia, KademliaConfig};
use particle_protocol::{ExtendedParticle, PROTOCOL_NAME};
use server_config::{Network, NetworkConfig};

use crate::connectivity::Connectivity;
use crate::health::{BootstrapNodesHealth, ConnectivityHealth, KademliaBootstrapHealth};

/// Coordinates protocols, so they can cooperate
#[derive(NetworkBehaviour)]
pub struct FluenceNetworkBehaviour {
    identify: Identify,
    ping: Ping,
    connection_limits: ConnectionLimits,
    pub(crate) connection_pool: ConnectionPoolBehaviour,
    pub(crate) kademlia: Kademlia,
}

struct KademliaConfigAdapter {
    peer_id: PeerId,
    network: Network,
    config: server_config::KademliaConfig,
}

#[derive(Error, Debug)]
pub enum ConfigConvertError {
    #[error("Failed to parse protocol name {raw}")]
    WrongProtocolName { raw: String },
}

impl TryFrom<KademliaConfigAdapter> for KademliaConfig {
    type Error = ConfigConvertError;
    fn try_from(value: KademliaConfigAdapter) -> Result<Self, Self::Error> {
        let protocol_name = format!(
            "/fluence/kad/{}/1.0.0",
            value.network.to_string().to_lowercase()
        );
        let protocol_name = StreamProtocol::try_from_owned(protocol_name.clone())
            .map_err(|_err| ConfigConvertError::WrongProtocolName { raw: protocol_name })?;
        Ok(Self {
            peer_id: value.peer_id,
            max_packet_size: value.config.max_packet_size,
            query_timeout: value.config.query_timeout,
            replication_factor: value.config.replication_factor,
            peer_fail_threshold: value.config.peer_fail_threshold,
            ban_cooldown: value.config.ban_cooldown,
            protocol_name,
        })
    }
}

impl FluenceNetworkBehaviour {
    pub fn new(
        network: Network,
        cfg: NetworkConfig,
        health_registry: Option<&mut HealthCheckRegistry>,
    ) -> eyre::Result<(Self, Connectivity, mpsc::Receiver<ExtendedParticle>)> {
        let local_public_key = cfg.key_pair.public();
        let identify = Identify::new(
            IdentifyConfig::new(PROTOCOL_NAME.into(), local_public_key)
                .with_agent_version(cfg.node_version.into()),
        );
        let ping = Ping::new(PingConfig::new());

        let kad_config = KademliaConfigAdapter {
            peer_id: cfg.local_peer_id,
            config: cfg.kademlia_config,
            network,
        };

        let kad_config = kad_config.try_into()?;
        let (kademlia, kademlia_api) = Kademlia::new(kad_config, cfg.libp2p_metrics);
        let (connection_pool, particle_stream, connection_pool_api) = ConnectionPoolBehaviour::new(
            cfg.particle_queue_buffer,
            cfg.protocol_config,
            cfg.local_peer_id,
            cfg.connection_pool_metrics,
        );

        let connection_limits = ConnectionLimits::new(cfg.connection_limits);

        let this = Self {
            kademlia,
            connection_pool,
            connection_limits,
            identify,
            ping,
        };

        let bootstrap_nodes = cfg.bootstrap_nodes.clone();

        let health = health_registry.map(|registry| {
            let bootstrap_nodes = BootstrapNodesHealth::new(bootstrap_nodes);
            let kademlia_bootstrap = KademliaBootstrapHealth::default();
            registry.register("bootstrap_nodes", bootstrap_nodes.clone());
            registry.register("kademlia_bootstrap", kademlia_bootstrap.clone());

            ConnectivityHealth {
                bootstrap_nodes,
                kademlia_bootstrap,
            }
        });

        let connectivity = Connectivity {
            peer_id: cfg.local_peer_id,
            kademlia: kademlia_api,
            connection_pool: connection_pool_api,
            bootstrap_nodes: cfg.bootstrap_nodes.into_iter().collect(),
            bootstrap_frequency: cfg.bootstrap_frequency,
            metrics: cfg.connectivity_metrics,
            health,
        };

        Ok((this, connectivity, particle_stream))
    }
}
