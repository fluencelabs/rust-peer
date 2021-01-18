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

use crate::behaviour::SwarmEventType;
use crate::ParticleBehaviour;

use particle_protocol::{HandlerMessage, ProtocolConfig};

use fluence_libp2p::{poll_loop, remote_multiaddr};

use libp2p::swarm::NetworkBehaviourAction;
use libp2p::{
    core::{
        connection::{ConnectionId, ListenerId},
        either::EitherOutput,
        ConnectedPoint, Multiaddr,
    },
    kad::{store::MemoryStore, Kademlia},
    swarm::{
        IntoProtocolsHandler, IntoProtocolsHandlerSelect, NetworkBehaviour,
        NetworkBehaviourEventProcess, OneShotHandler, PollParameters, ProtocolsHandler,
    },
    PeerId,
};
use std::ops::DerefMut;
use std::{
    error::Error,
    task::{Context, Poll},
};

impl NetworkBehaviour for ParticleBehaviour {
    type ProtocolsHandler = IntoProtocolsHandlerSelect<
        OneShotHandler<ProtocolConfig, HandlerMessage, HandlerMessage>,
        <Kademlia<MemoryStore> as NetworkBehaviour>::ProtocolsHandler,
    >;
    type OutEvent = ();

    fn new_handler(&mut self) -> Self::ProtocolsHandler {
        IntoProtocolsHandler::select(
            self.connection_pool.new_handler(),
            self.kademlia.new_handler(),
        )
    }

    fn addresses_of_peer(&mut self, peer_id: &PeerId) -> Vec<Multiaddr> {
        let p = self.connection_pool.addresses_of_peer(peer_id).into_iter();
        let d = self.kademlia.addresses_of_peer(peer_id).into_iter();

        p.chain(d).collect()
    }

    fn inject_connected(&mut self, peer_id: &PeerId) {
        self.connection_pool.inject_connected(peer_id);
        self.kademlia.inject_connected(peer_id);
    }

    fn inject_disconnected(&mut self, peer_id: &PeerId) {
        self.connection_pool.inject_disconnected(peer_id);
        self.kademlia.inject_disconnected(peer_id);
    }

    fn inject_event(
        &mut self,
        peer_id: PeerId,
        connection: ConnectionId,
        event: <<Self::ProtocolsHandler as IntoProtocolsHandler>::Handler as ProtocolsHandler>::OutEvent,
    ) {
        use EitherOutput::{First, Second};

        match event {
            First(event) => NetworkBehaviour::inject_event(
                &mut self.connection_pool,
                peer_id,
                connection,
                event,
            ),
            Second(event) => {
                NetworkBehaviour::inject_event(&mut self.kademlia, peer_id, connection, event)
            }
        }
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        params: &mut impl PollParameters,
    ) -> Poll<SwarmEventType> {
        self.waker = Some(cx.waker().clone());

        let kad_ready = self.kademlia.poll(cx, params).is_ready();
        let pool_ready = self.connection_pool.poll(cx, params).is_ready();

        if kad_ready || pool_ready {
            return Poll::Ready(NetworkBehaviourAction::GenerateEvent(()));
        }

        Poll::Pending
    }

    // ==== tedious repetition below ====
    fn inject_addr_reach_failure(
        &mut self,
        peer_id: Option<&PeerId>,
        addr: &Multiaddr,
        error: &dyn Error,
    ) {
        self.connection_pool
            .inject_addr_reach_failure(peer_id, addr, error);
        self.kademlia
            .inject_addr_reach_failure(peer_id, addr, error);
    }

    fn inject_dial_failure(&mut self, peer_id: &PeerId) {
        self.connection_pool.inject_dial_failure(peer_id);
        self.kademlia.inject_dial_failure(peer_id);
    }

    fn inject_new_listen_addr(&mut self, addr: &Multiaddr) {
        self.connection_pool.inject_new_listen_addr(addr);
        self.kademlia.inject_new_listen_addr(addr);
    }

    fn inject_expired_listen_addr(&mut self, addr: &Multiaddr) {
        self.connection_pool.inject_expired_listen_addr(addr);
        self.kademlia.inject_expired_listen_addr(addr);
    }

    fn inject_new_external_addr(&mut self, addr: &Multiaddr) {
        self.connection_pool.inject_new_external_addr(addr);
        self.kademlia.inject_new_external_addr(addr);
    }

    fn inject_listener_error(&mut self, id: ListenerId, err: &(dyn std::error::Error + 'static)) {
        self.connection_pool.inject_listener_error(id, err);
        self.kademlia.inject_listener_error(id, err);
    }

    fn inject_listener_closed(&mut self, id: ListenerId, reason: Result<(), &std::io::Error>) {
        self.connection_pool.inject_listener_closed(id, reason);
        self.kademlia.inject_listener_closed(id, reason);
    }

    fn inject_connection_established(
        &mut self,
        id: &PeerId,
        ci: &ConnectionId,
        cp: &ConnectedPoint,
    ) {
        self.connection_pool
            .inject_connection_established(id, ci, cp);
        self.kademlia.inject_connection_established(id, ci, cp);
    }

    fn inject_connection_closed(&mut self, id: &PeerId, ci: &ConnectionId, cp: &ConnectedPoint) {
        self.connection_pool.inject_connection_closed(id, ci, cp);
        self.kademlia.inject_connection_closed(id, ci, cp);
    }

    fn inject_address_change(
        &mut self,
        id: &PeerId,
        ci: &ConnectionId,
        old: &ConnectedPoint,
        new: &ConnectedPoint,
    ) {
        self.connection_pool.inject_address_change(id, ci, old, new);
        self.kademlia.inject_address_change(id, ci, old, new);
    }
}
