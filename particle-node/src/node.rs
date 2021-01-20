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

use super::behaviour::NetworkBehaviour;

use config_utils::to_peer_id;
use fluence_libp2p::{build_transport, types::OneshotOutlet};
use server_config::{BehaviourConfig, NodeConfig};
use trust_graph::TrustGraph;

use anyhow::Context;
use async_std::sync::Mutex;
use async_std::task;
use fluence_libp2p::types::{BackPressuredInlet, BackPressuredOutlet};
use futures::{channel::oneshot, future::BoxFuture, select, stream::StreamExt, FutureExt, SinkExt};
use libp2p::swarm::{AddressScore, ExpandedSwarm};
use libp2p::{
    core::{multiaddr::Protocol, Multiaddr},
    identity::ed25519::Keypair,
    PeerId, Swarm, TransportError,
};
use particle_protocol::Particle;
use prometheus::Registry;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::task::Poll;

pub mod unlocks {
    use async_std::sync::Mutex;
    use futures::Future;
    use std::ops::DerefMut;

    pub async fn unlock_f<T, R, F: Future<Output = R>>(
        m: &Mutex<T>,
        f: impl FnOnce(&mut T) -> F,
    ) -> R {
        unlock(m, f).await.await
    }

    pub async fn unlock<T, R>(m: &Mutex<T>, f: impl FnOnce(&mut T) -> R) -> R {
        let mut guard = m.lock().await;
        let result = f(guard.deref_mut());
        drop(guard);
        result
    }
}

use crate::behaviour::NetworkApi;
use crate::execute_effect;
use crate::metrics::start_metrics_endpoint;
use crate::network_api::NetworkApi;
use async_std::task::JoinHandle;
use futures::channel::mpsc;
use futures::channel::oneshot::Canceled;
use particle_actors::{StepperEffects, StepperPoolApi, StepperPoolProcessor, VmPoolConfig};
use particle_closures::{HostClosures, NodeInfo};

// TODO: documentation
pub struct Node {
    network_api: NetworkApi,
    swarm: Swarm<NetworkBehaviour>,
    stepper_pool: StepperPoolProcessor,
    stepper_pool_api: StepperPoolApi,
    // TODO: split config into several parts to avoid clones
    //       narrow scope of the configuration that is stored in Node
    //       cut parts of the ServerConfig along with creation of corresponding components
    //       e.g., no need to store services_base_dir and services_envs in Node – split it to separate config
    //       and move it to HostClosures on creation. Same with VmPoolConfig.
    config: NodeConfig,
    local_peer_id: PeerId,
    registry: Registry,
}

impl Node {
    pub fn new(key_pair: Keypair, config: NodeConfig) -> anyhow::Result<Box<Self>> {
        let NodeConfig { socket_timeout, .. } = config;

        let local_peer_id = to_peer_id(&key_pair);
        log::info!("server peer id = {}", local_peer_id);

        let trust_graph = TrustGraph::new(config.root_weights());
        let registry = Registry::new();

        let (mut swarm, network_api) = {
            let config =
                BehaviourConfig::new(trust_graph, Some(&registry), key_pair.clone(), &config);
            let (behaviour, network_api) =
                NetworkBehaviour::new(config).context("failed to crate ServerBehaviour")?;
            let key_pair = libp2p::identity::Keypair::Ed25519(key_pair);
            let transport = build_transport(key_pair, socket_timeout);
            let swarm = Swarm::new(transport, behaviour, local_peer_id);

            (swarm, network_api)
        };

        // Add external addresses to Swarm
        config.external_addresses().into_iter().for_each(|addr| {
            Swarm::add_external_address(&mut swarm, addr, AddressScore::Finite(1));
        });

        let pool_config = VmPoolConfig::new(
            local_peer_id,
            config.stepper_base_dir.clone(),
            config.air_interpreter_path.clone(),
            config.stepper_pool_size.clone(),
        )
        .expect("create vm pool config");

        let node_info = NodeInfo {
            external_addresses: config.external_addresses(),
        };
        let host_closures = HostClosures::new(
            node_info,
            local_peer_id,
            config.services_base_dir.clone(),
            config.services_envs.clone(),
        )
        .expect("create host closures");

        let (stepper_pool, stepper_pool_api) =
            StepperPoolProcessor::new(pool_config, host_closures.descriptor());

        let node_service = Self {
            network_api,
            swarm,
            stepper_pool,
            stepper_pool_api,
            config,
            local_peer_id,
            registry,
        };

        Ok(Box::new(node_service))
    }

    /// Starts node service
    pub fn start(mut self: Box<Self>) -> OneshotOutlet<()> {
        let (exit_outlet, exit_inlet) = oneshot::channel();
        let mut exit_inlet = exit_inlet.into_stream().fuse();

        self.listen().expect("Error on starting node listener");
        // self.swarm.dial_bootstrap_nodes();

        task::spawn(async move {
            let mut metrics =
                start_metrics_endpoint(self.registry, self.config.metrics_listen_addr()).fuse();

            let pool = self.stepper_pool.start();
            let network = self.network_api.start(
                self.stepper_pool_api,
                self.config.particle_processor_parallelism,
            );
            loop {
                select!(
                    _ = self.swarm.select_next_some() => {},
                    _ = metrics => {},
                    event = exit_inlet.next() => {
                        // Ignore Err and None – if exit_outlet is dropped, we'll run forever!
                        if let Some(Ok(_)) = event {
                            break
                        }
                    }
                )
            }

            log::info!("Stopping node");
            network.cancel().await;
            pool.cancel().await;
        });

        exit_outlet
    }

    /// Starts node service listener.
    #[inline]
    fn listen(&mut self) -> Result<(), TransportError<io::Error>> {
        let mut tcp = Multiaddr::from(self.config.listen_ip);
        tcp.push(Protocol::Tcp(self.config.tcp_port));

        let mut ws = Multiaddr::from(self.config.listen_ip);
        ws.push(Protocol::Tcp(self.config.websocket_port));
        ws.push(Protocol::Ws("/".into()));

        log::info!("Fluence listening on {} and {}", tcp, ws);

        Swarm::listen_on(&mut self.swarm, tcp)?;
        Swarm::listen_on(&mut self.swarm, ws)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::Node;
    use ctrlc_adapter::block_until_ctrlc;
    use fluence_libp2p::RandomPeerId;
    use libp2p::core::connection::ConnectionId;
    use libp2p::core::Multiaddr;
    use libp2p::identity::ed25519::Keypair;
    use libp2p::swarm::NetworkBehaviour;
    use libp2p::Swarm;
    use maplit::hashmap;
    use particle_protocol::{HandlerMessage, Particle};
    use serde_json::json;
    use server_config::{deserialize_config, NodeConfig};
    use std::path::PathBuf;
    use test_utils::enable_logs;
    use test_utils::ConnectedClient;

    #[test]
    fn run_node() {
        enable_logs();

        let keypair = Keypair::generate();

        let config = std::fs::read("../deploy/Config.default.toml").expect("find default config");
        let mut config = deserialize_config(<_>::default(), config).expect("deserialize config");
        config.server.stepper_pool_size = 1;
        config.server.air_interpreter_path = PathBuf::from("../aquamarine_0.0.30.wasm");
        let mut node = Node::new(keypair, config.server).unwrap();

        Box::new(node).start();

        let listening_address: Multiaddr = "/ip4/127.0.0.1/tcp/7777".parse().unwrap();
        let mut client = ConnectedClient::connect_to(listening_address).expect("connect client");
        println!("client: {}", client.peer_id);
        let data = hashmap! {
            "name" => json!("folex"),
            "client" => json!(client.peer_id.to_string()),
            "relay" => json!(client.node.to_string()),
        };
        client.send_particle(
            r#"
                (seq
                    (call relay ("op" "identity") [] void[])
                    (call client ("return" "") [name] void[])
                )
            "#,
            data.clone(),
        );
        let response = client.receive_args();
        println!("got response!: {:#?}", response);

        // block_until_ctrlc();
    }
}
