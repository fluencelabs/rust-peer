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

use crate::config::PeerServiceConfig;
use crate::peer_service::{
    behaviour::PeerServiceBehaviour,
    notifications::{InPeerNotification, OutPeerNotification},
    transport::build_transport,
    transport::PeerServiceTransport,
};
use async_std::task;
use futures::{channel::mpsc, stream::StreamExt};
use libp2p::{
    core::muxing::{StreamMuxerBox, SubstreamRef},
    identity, PeerId, Swarm,
};
use log::trace;
use parity_multiaddr::{Multiaddr, Protocol};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

pub struct PeerService {
    pub swarm:
        Box<Swarm<PeerServiceTransport, PeerServiceBehaviour<SubstreamRef<Arc<StreamMuxerBox>>>>>,
}

impl PeerService {
    pub fn new(config: PeerServiceConfig) -> Arc<Mutex<Self>> {
        let local_key = identity::Keypair::generate_ed25519();
        let local_peer_id = PeerId::from(local_key.public());
        println!("peer service is starting with id = {}", local_peer_id);

        let mut swarm = {
            let transport = build_transport(local_key.clone(), config.socket_timeout);
            let behaviour = PeerServiceBehaviour::new(&local_peer_id, local_key.public());

            Box::new(Swarm::new(transport, behaviour, local_peer_id))
        };

        let mut listen_addr = Multiaddr::from(config.listen_ip);
        listen_addr.push(Protocol::Tcp(config.listen_port));
        Swarm::listen_on(&mut swarm, listen_addr).unwrap();

        Arc::new(Mutex::new(Self { swarm }))
    }
}

pub fn start_peer_service(
    peer_service: Arc<Mutex<PeerService>>,
    mut peer_service_in_receiver: mpsc::UnboundedReceiver<InPeerNotification>,
    peer_service_out_sender: mpsc::UnboundedSender<OutPeerNotification>,
) -> task::JoinHandle<()> {
    let handle = task::spawn(futures::future::poll_fn(move |cx: &mut Context| {
        println!("peer service loop");
        loop {
            match peer_service_in_receiver.poll_next_unpin(cx) {
                Poll::Ready(Some(e)) => match e {
                    InPeerNotification::Relay {
                        src_id,
                        dst_id,
                        data,
                    } => peer_service
                        .lock()
                        .unwrap()
                        .swarm
                        .relay_message(src_id, dst_id, data),

                    InPeerNotification::NetworkState { dst_id, state } => peer_service
                        .lock()
                        .unwrap()
                        .swarm
                        .send_network_state(dst_id, state),
                },
                Poll::Pending => {
                    println!("pending");
                    break;
                }
                Poll::Ready(None) => {
                    println!("None");
                    // TODO: propagate error
                    break;
                }
            }
        }

        println!("before swarm");

        loop {
            match peer_service.lock().unwrap().swarm.poll_next_unpin(cx) {
                Poll::Ready(Some(e)) => {
                    trace!("peer_service/poll: received {:?} event", e);
                }
                Poll::Ready(None) => unreachable!("stream never ends"),
                Poll::Pending => break,
            }
        }

        println!("11");

        if let Some(e) = peer_service.lock().unwrap().swarm.pop_out_node_event() {
            trace!("peer_service/poll: sending {:?} to peer_service", e);

            peer_service_out_sender.unbounded_send(e).unwrap();
        }

        println!("2");

        Poll::Pending
    }));

    handle
}
