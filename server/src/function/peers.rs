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

use super::wait_peer::WaitPeer;
use super::FunctionRouter;
use crate::function::waiting_queues::Enqueued;
use faas_api::{FunctionCall, Protocol};
use libp2p::{
    swarm::{DialPeerCondition, NetworkBehaviour, NetworkBehaviourAction},
    PeerId,
};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum PeerStatus {
    /// Peer is expected to be connected
    Connected = 3000,
    /// Peer is expected to be in the routing table
    Routable = 2000,
    /// State of the peer is unknown, need to look it up in kademlia
    Unknown = 1000,
    /// When peer is routable, but could be missing from the routing table.
    /// "Checked" here means that state was checked externally, i.e., via GetClosestPeers
    /// It is the lowest of all PeerStatus'es, because it is impossible to check in `send_to`
    CheckedRoutable = 0,
}

// Contains methods related to searching for and connecting to peers
impl FunctionRouter {
    /// Look for peer in Kademlia, enqueue call to wait for result
    pub(super) fn search_then_send(&mut self, peer_id: PeerId, call: FunctionCall) {
        self.query_closest(peer_id, WaitPeer::Routable(call))
    }

    pub(super) fn send_to_neighborhood(&mut self, peer_id: PeerId, call: FunctionCall) {
        self.query_closest(peer_id, WaitPeer::Neighborhood(call))
    }

    /// Query for peers closest to the `peer_id` as DHT key, enqueue call until response
    fn query_closest(&mut self, peer_id: PeerId, call: WaitPeer) {
        // Don't call get_closest_peers if there are already some calls waiting for it
        if let Enqueued::New = self.wait_peer.enqueue(peer_id.clone(), call) {
            // NOTE: Using Qm form of `peer_id` here (via peer_id.borrow), since kademlia uses that for keys
            self.kademlia.get_closest_peers(peer_id);
        }
    }

    /// Send all calls waiting for this peer to be found
    pub(super) fn found_closest(&mut self, key: Vec<u8>, peers: Vec<PeerId>) {
        let peer_id = match PeerId::from_bytes(key) {
            Err(err) => {
                log::warn!(
                    "Found closest peers for invalid key {}: not a PeerId",
                    bs58::encode(err).into_string()
                );
                return;
            }
            Ok(peer_id) => peer_id,
        };

        if self.is_local(&peer_id) {
            self.bootstrap_finished();
        }

        // Forward to `peer_id`
        let calls = self.wait_peer.remove_with(peer_id.clone(), |wp| wp.found());
        for call in calls {
            if peers.is_empty() || !peers.iter().any(|p| p == &peer_id) {
                let err_msg = format!(
                    "peer {} wasn't found via closest query: {:?}",
                    peer_id,
                    peers.iter().map(|p| p.to_base58())
                );
                log::warn!("{}", err_msg);
                // Peer wasn't found, send error
                self.send_error_on_call(call.into(), err_msg)
            } else {
                // Forward calls to `peer_id`, guaranteeing it is now routable
                self.send_to(
                    peer_id.clone(),
                    PeerStatus::CheckedRoutable,
                    call.into(),
                    "peer was found in kademlia",
                )
            }
        }

        // Forward to neighborhood
        let calls = self.wait_peer.remove_with(peer_id, |wp| wp.neighborhood());
        for call in calls {
            let call: FunctionCall = call.into();

            // Check if any peers found, if not – send error
            if peers.is_empty() {
                self.send_error_on_call(call, "neighborhood was empty".into());
                return;
            }

            // Forward calls to each peer in neighborhood
            for peer_id in peers.iter() {
                let mut call = call.clone();
                // Modify target: prepend peer
                let target =
                    Protocol::Peer(peer_id.clone()) / call.target.take().unwrap_or_default();
                self.send_to(
                    peer_id.clone(),
                    // Peer was found, so it must be routable
                    PeerStatus::CheckedRoutable,
                    call.with_target(target),
                    "peer was found in neighborhood",
                )
            }
        }
    }

    pub(super) fn connect_then_send(&mut self, peer_id: PeerId, call: FunctionCall) {
        use DialPeerCondition::Disconnected as condition; // o_O you can do that?!
        use NetworkBehaviourAction::DialPeer;
        use WaitPeer::Connected;

        self.wait_peer.enqueue(peer_id.clone(), Connected(call));

        log::info!("Dialing {}", peer_id);
        self.events.push_back(DialPeer { peer_id, condition });
    }

    pub(super) fn connected(&mut self, peer_id: PeerId) {
        log::info!("Peer connected: {}", peer_id);
        self.connected_peers.insert(peer_id.clone());

        let waiting = self
            .wait_peer
            .remove_with(peer_id.clone(), |wp| wp.connected());

        // TODO: leave move or remove move from closure?
        waiting.for_each(move |wp| match wp {
            WaitPeer::Connected(call) => self.send_to_connected(peer_id.clone(), call),
            _ => unreachable!("Can't happen. Just filtered WaitPeer::Connected"),
        });
    }

    // TODO: clear connected_peers on inject_listener_closed?
    pub(super) fn disconnected(&mut self, peer_id: &PeerId) {
        log::info!(
            "Peer disconnected: {}. {} calls left waiting.",
            peer_id,
            self.wait_peer.count(&peer_id)
        );
        self.connected_peers.remove(peer_id);

        self.remove_halted_names(peer_id);

        let waiting_calls = self.wait_peer.remove(peer_id);
        for waiting in waiting_calls.into_iter() {
            self.send_error_on_call(waiting.into(), format!("peer {} disconnected", peer_id));
        }
    }

    pub(super) fn is_connected(&self, peer_id: &PeerId) -> bool {
        self.connected_peers.contains(peer_id)
    }

    /// Whether peer is in the routing table
    pub(super) fn is_routable(&mut self, peer_id: &PeerId) -> bool {
        // TODO (only relevant to local clients):
        //       Is it possible for a client to be routable via swarm, but not via kademlia?
        //       If so, need to ask swarm if client is routable
        let in_kad = !self.kademlia.addresses_of_peer(peer_id).is_empty();
        log::debug!("peer {} in routing table? {:?}", peer_id, in_kad);
        in_kad
    }

    /// Whether given peer id is equal to ours
    pub(super) fn is_local(&self, peer_id: &PeerId) -> bool {
        if self.config.peer_id.eq(peer_id) {
            log::debug!("{} is LOCAL", peer_id);
            true
        } else {
            log::debug!("{} is REMOTE", peer_id);
            false
        }
    }

    pub(super) fn search_for_client(&mut self, client: PeerId, call: FunctionCall) {
        self.find_providers(Protocol::Client(client).into(), call)
    }

    pub(super) fn peer_status(&mut self, peer: &PeerId) -> PeerStatus {
        use PeerStatus::*;

        if self.is_connected(peer) {
            Connected
        } else if self.is_routable(peer) {
            Routable
        } else {
            Unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_status() {
        use PeerStatus::*;

        assert!(Connected > Unknown);
        assert!(Connected > Routable);
        assert!(Connected > CheckedRoutable);
        assert_eq!(Connected, Connected);

        assert!(Routable > Unknown);
        assert!(Routable > CheckedRoutable);
        assert!(Routable < Connected);
        assert_eq!(Routable, Routable);

        assert!(Unknown > CheckedRoutable);
        assert!(Unknown < Connected);
        assert!(Unknown < Routable);
        assert_eq!(Unknown, Unknown);

        assert!(CheckedRoutable < Connected);
        assert!(CheckedRoutable < Routable);
        assert!(CheckedRoutable < Unknown);
        assert_eq!(CheckedRoutable, CheckedRoutable);
    }
}
