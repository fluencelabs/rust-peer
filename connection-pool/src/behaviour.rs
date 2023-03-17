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

use futures::{Sink, StreamExt};
use libp2p::swarm::dial_opts::DialOpts;
use libp2p::swarm::CloseConnection::All;
use libp2p::swarm::{ConnectionId, dial_opts, DialError, FromSwarm, THandlerOutEvent};
use libp2p::{
    core::{ConnectedPoint, Multiaddr},
    swarm::{
        NetworkBehaviour, NetworkBehaviourAction, NotifyHandler, OneShotHandler,
        PollParameters,
    },
    PeerId,
};
use std::pin::Pin;
use std::{
    collections::{hash_map::Entry, HashMap, HashSet, VecDeque},
    task::{Context, Poll, Waker},
};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_util::sync::PollSender;

use fluence_libp2p::remote_multiaddr;
use particle_protocol::{
    CompletionChannel, Contact, HandlerMessage, Particle, ProtocolConfig, SendStatus,
};
use peer_metrics::ConnectionPoolMetrics;

use crate::connection_pool::LifecycleEvent;
use crate::{Command, ConnectionPoolApi};

// type SwarmEventType = generate_swarm_event_type!(ConnectionPoolBehaviour);

// TODO: replace with generate_swarm_event_type
type SwarmEventType = NetworkBehaviourAction<(), HandlerMessage>;

#[derive(Debug, Default)]
/// [Peer] is the representation of [Contact] extended with precise connectivity information
struct Peer {
    /// Current peer has active connections with that list of addresses
    connected: HashSet<Multiaddr>,
    /// Addresses gathered via Identify protocol, but not connected
    discovered: HashSet<Multiaddr>,
    /// Dialed but not yet connected addresses
    dialing: HashSet<Multiaddr>,
    /// Channels to notify when any dial succeeds or peer is already connected
    dial_promises: Vec<oneshot::Sender<bool>>,
    // TODO: this layout of `dialing` and `dial_promises` doesn't allow to check specific addresses for reachability
    //       if check reachability for specific maddrs is ever required, one would need to maintain the following info:
    //       reachability_promises: HashMap<Multiaddr, Vec<oneshot::Sender<bool>>
}

impl Peer {
    pub fn addresses(&self) -> impl Iterator<Item=&Multiaddr> {
        self.connected
            .iter()
            .chain(&self.discovered)
            .chain(&self.dialing)
            .collect::<HashSet<_>>()
            .into_iter()
    }

    pub fn connected(addresses: impl IntoIterator<Item=Multiaddr>) -> Self {
        Peer {
            connected: addresses.into_iter().collect(),
            discovered: Default::default(),
            dialing: Default::default(),
            dial_promises: vec![],
        }
    }

    pub fn dialing(
        addresses: impl IntoIterator<Item=Multiaddr>,
        outlet: oneshot::Sender<bool>,
    ) -> Self {
        Peer {
            connected: Default::default(),
            discovered: Default::default(),
            dialing: addresses.into_iter().collect(),
            dial_promises: vec![outlet],
        }
    }
}

pub struct ConnectionPoolBehaviour {
    peer_id: PeerId,

    commands: UnboundedReceiverStream<Command>,

    outlet: PollSender<Particle>,
    subscribers: Vec<mpsc::UnboundedSender<LifecycleEvent>>,

    queue: VecDeque<Particle>,
    contacts: HashMap<PeerId, Peer>,
    dialing: HashMap<Multiaddr, Vec<oneshot::Sender<Option<Contact>>>>,

    events: VecDeque<SwarmEventType>,
    waker: Option<Waker>,
    pub(super) protocol_config: ProtocolConfig,

    metrics: Option<ConnectionPoolMetrics>,
}

impl ConnectionPoolBehaviour {
    fn execute(&mut self, cmd: Command) {
        match cmd {
            Command::Dial { addr, out } => self.dial(addr, out),
            Command::Connect { contact, out } => self.connect(contact, out),
            Command::Disconnect { peer_id, out } => self.disconnect(peer_id, out),
            Command::IsConnected { peer_id, out } => self.is_connected(peer_id, out),
            Command::GetContact { peer_id, out } => self.get_contact(peer_id, out),
            Command::Send { to, particle, out } => self.send(to, particle, out),
            Command::CountConnections { out } => self.count_connections(out),
            Command::LifecycleEvents { out } => self.add_subscriber(out),
        }
    }

    /// Dial `address`, and send contact back on success
    /// `None` means something prevented us from connecting - dial reach failure or something else
    pub fn dial(&mut self, address: Multiaddr, out: oneshot::Sender<Option<Contact>>) {
        // TODO: return Contact immediately if that address is already connected
        self.dialing.entry(address.clone()).or_default().push(out);

        self.push_event(NetworkBehaviourAction::Dial {
            opts: DialOpts::unknown_peer_id().address(address).build()
        });
    }

    /// Connect to the contact by all of its known addresses and return whether connection succeeded
    /// If contact is already being dialed and there are no new addresses in Contact, don't dial
    /// If contact is already connected, return `true` immediately
    pub fn connect(&mut self, new_contact: Contact, outlet: oneshot::Sender<bool>) {
        let addresses = match self.contacts.entry(new_contact.peer_id) {
            Entry::Occupied(mut entry) => {
                let known_contact = entry.get_mut();

                // collect previously unknown addresses
                let mut new_addrs = HashSet::new();
                // flag if `contact` has any unconnected addresses
                let mut not_connected = false;
                for maddr in new_contact.addresses {
                    if !known_contact.connected.contains(&maddr) {
                        not_connected = true;
                    }

                    if !known_contact.dialing.contains(&maddr) {
                        new_addrs.insert(maddr);
                    }
                }

                if not_connected {
                    // we got either new addresses to dial, or in-progress dialing on some
                    // addresses in `new_contact`, so remember to notify channel about dial state change
                    known_contact.dial_promises.push(outlet);
                } else {
                    // all addresses in `new_contact` are already connected, so notify about success
                    outlet.send(true).ok();
                }
                new_addrs.into_iter().collect()
            }
            Entry::Vacant(slot) => {
                slot.insert(Peer::dialing(new_contact.addresses.clone(), outlet));
                new_contact.addresses
            }
        };

        if !addresses.is_empty() {
            self.push_event(NetworkBehaviourAction::Dial {
                opts: DialOpts::peer_id(new_contact.peer_id)
                    .addresses(addresses)
                    .build()
            });
        }
    }

    pub fn disconnect(&mut self, peer_id: PeerId, outlet: oneshot::Sender<bool>) {
        self.push_event(NetworkBehaviourAction::CloseConnection {
            peer_id,
            connection: All,
        });
        // TODO: signal disconnect completion only after `peer_removed` was called or Disconnect failed
        outlet.send(true).ok();
    }

    /// Returns whether given peer is connected or not
    pub fn is_connected(&self, peer_id: PeerId, outlet: oneshot::Sender<bool>) {
        outlet.send(self.contacts.contains_key(&peer_id)).ok();
    }

    /// Returns contact for a given peer if it is known
    pub fn get_contact(&self, peer_id: PeerId, outlet: oneshot::Sender<Option<Contact>>) {
        let contact = self.get_contact_impl(peer_id);
        outlet.send(contact).ok();
    }

    /// Sends a particle to a connected contact. Returns whether sending succeeded or not
    /// Result is sent to channel inside `upgrade_outbound` in ProtocolHandler
    pub fn send(&mut self, to: Contact, particle: Particle, outlet: oneshot::Sender<SendStatus>) {
        if to.peer_id == self.peer_id {
            // If particle is sent to the current node, process it locally
            self.queue.push_back(particle);
            outlet.send(SendStatus::Ok).ok();
            self.wake();
        } else if self.contacts.contains_key(&to.peer_id) {
            log::debug!(target: "network", "{}: Sending particle {} to {}", self.peer_id, particle.id, to.peer_id);
            // Send particle to remote peer
            self.push_event(NetworkBehaviourAction::NotifyHandler {
                peer_id: to.peer_id,
                handler: NotifyHandler::Any,
                event: HandlerMessage::OutParticle(particle, CompletionChannel::Oneshot(outlet)),
            });
        } else {
            log::warn!(
                "Won't send particle {} to contact {}: not connected",
                particle.id,
                to.peer_id
            );
            outlet.send(SendStatus::NotConnected).ok();
        }
    }

    /// Returns number of connected contacts
    pub fn count_connections(&mut self, outlet: oneshot::Sender<usize>) {
        outlet.send(self.contacts.len()).ok();
    }

    /// Subscribes given channel for all `LifecycleEvent`s
    pub fn add_subscriber(&mut self, outlet: mpsc::UnboundedSender<LifecycleEvent>) {
        self.subscribers.push(outlet);
    }

    pub fn add_discovered_addresses(&mut self, peer_id: PeerId, addresses: Vec<Multiaddr>) {
        self.contacts
            .entry(peer_id)
            .or_default()
            .discovered
            .extend(addresses);
    }

    fn meter<U, F: Fn(&ConnectionPoolMetrics) -> U>(&self, f: F) {
        self.metrics.as_ref().map(f);
    }
}

impl ConnectionPoolBehaviour {
    pub fn new(
        buffer: usize,
        protocol_config: ProtocolConfig,
        peer_id: PeerId,
        metrics: Option<ConnectionPoolMetrics>,
    ) -> (Self, mpsc::Receiver<Particle>, ConnectionPoolApi) {
        let (outlet, inlet) = mpsc::channel(buffer);
        let outlet = PollSender::new(outlet);
        let (command_outlet, command_inlet) = mpsc::unbounded_channel();
        let api = ConnectionPoolApi {
            outlet: command_outlet,
            send_timeout: protocol_config.upgrade_timeout * 2,
        };

        let this = Self {
            peer_id,
            outlet,
            commands: UnboundedReceiverStream::new(command_inlet),
            subscribers: <_>::default(),
            queue: <_>::default(),
            contacts: <_>::default(),
            dialing: <_>::default(),
            events: <_>::default(),
            waker: None,
            protocol_config,
            metrics,
        };

        (this, inlet, api)
    }

    fn wake(&self) {
        if let Some(waker) = &self.waker {
            waker.wake_by_ref();
        }
    }

    fn add_connected_address(&mut self, peer_id: PeerId, maddr: Multiaddr) {
        // notify these waiting for a peer to be connected
        match self.contacts.entry(peer_id) {
            Entry::Occupied(mut entry) => {
                let peer = entry.get_mut();
                peer.dialing.remove(&maddr);
                peer.discovered.remove(&maddr);
                peer.connected.insert(maddr.clone());

                let dial_promises = std::mem::take(&mut peer.dial_promises);

                for out in dial_promises {
                    out.send(true).ok();
                }
            }
            Entry::Vacant(e) => {
                e.insert(Peer::connected(std::iter::once(maddr.clone())));
            }
        }

        // notify these waiting for an address to be dialed
        if let Some(outs) = self.dialing.remove(&maddr) {
            let contact = self.get_contact_impl(peer_id);
            debug_assert!(contact.is_some());
            for out in outs {
                out.send(contact.clone()).ok();
            }
        }
        let connected_peers = i64::try_from(self.contacts.len());
        match connected_peers {
            Ok(connected_peers) => {
                self.meter(|m| m.connected_peers.set(connected_peers));
            }
            Err(e) => log::warn!("Could not convert metric connected_peers {}", e),
        }
    }

    fn lifecycle_event(&mut self, event: LifecycleEvent) {
        self.subscribers.retain(|out| {
            let ok = out.send(event.clone());
            ok.is_ok()
        })
    }

    fn push_event(&mut self, event: SwarmEventType) {
        self.events.push_back(event);
        self.wake();
    }

    fn remove_contact(&mut self, peer_id: &PeerId, reason: &str) {
        if let Some(contact) = self.contacts.remove(peer_id) {
            log::debug!("Contact {} was removed: {}", peer_id, reason);
            self.lifecycle_event(LifecycleEvent::Disconnected(Contact::new(
                *peer_id,
                contact.addresses().cloned().collect(),
            )));

            for out in contact.dial_promises {
                // if dial was in progress, notify waiters
                out.send(false).ok();
            }

            let connected_peers = i64::try_from(self.contacts.len());
            match connected_peers {
                Ok(connected_peers) => {
                    self.meter(|m| m.connected_peers.set(connected_peers));
                }
                Err(e) => log::warn!("Could not convert metric connected_peers {}", e),
            }
        }
    }

    fn get_contact_impl(&self, peer_id: PeerId) -> Option<Contact> {
        self.contacts.get(&peer_id).map(|c| Contact {
            peer_id,
            addresses: c.addresses().cloned().collect(),
        })
    }

    fn on_connection_established(
        &mut self,
        peer_id: &PeerId,
        cp: &ConnectedPoint,
        failed_addresses: &Vec<Multiaddr>,
    ) {
        // mark failed addresses as such
        for addr in failed_addresses {
            log::warn!("failed to connect to {} {}", addr, peer_id);

            self.cleanup_address(Some(peer_id), addr)
        }

        let multiaddr = remote_multiaddr(cp).clone();
        log::debug!(
            target: "network",
            "{}: connection established with {} @ {}",
            self.peer_id,
            peer_id,
            multiaddr
        );

        self.add_connected_address(*peer_id, multiaddr.clone());

        self.lifecycle_event(LifecycleEvent::Connected(Contact::new(
            *peer_id,
            vec![multiaddr],
        )))
    }

    fn on_connection_closed(
        &mut self,
        peer_id: &PeerId,
        cp: &ConnectedPoint,
        remaining_established: usize,
    ) {
        let multiaddr = remote_multiaddr(cp);
        if remaining_established == 0 {
            self.remove_contact(peer_id, "disconnected");
            log::debug!(
                target: "network",
                "{}: connection lost with {} @ {}",
                self.peer_id,
                peer_id,
                multiaddr
            );
        } else {
            log::debug!(
                target: "network",
                "{}: {} connections remaining established with {}. {} has just closed.",
                self.peer_id,
                remaining_established,
                peer_id,
                multiaddr
            )
        }

        self.cleanup_address(Some(peer_id), multiaddr);
    }

    fn on_dial_failure(
        &mut self,
        peer_id: Option<PeerId>,
        error: &DialError,
    ) {
        use dial_opts::PeerCondition::{Disconnected, NotDialing};
        if let DialError::DialPeerConditionFalse(Disconnected | NotDialing) = error {
            // So, if you tell libp2p to dial a peer, there's an option dial_opts::PeerCondition
            // The default one is Disconnected.
            // So, if you asked libp2p to connect to a peer, and the peer IS ALREADY CONNECTED,
            // libp2p will tell you that dial has failed.
            // We need to ignore this "failure" in case condition is Disconnected or NotDialing.
            // Because this basically means that peer has already connected while our Dial was processed.
            // That could happen in several cases:
            //  1. `dial` was called by multiaddress of an already-connected peer
            //  2. `connect` was called with new multiaddresses, but target peer is already connected
            //  3. unknown data race
            log::info!("Dialing attempt to an already connected peer {:?}", peer_id);
            return;
        }

        log::warn!(
            "Error dialing peer {}: {:?}",
            peer_id.map_or("unknown".to_string(), |id| id.to_string()),
            error
        );
        match error {
            DialError::WrongPeerId { endpoint, .. } => {
                let addr = match endpoint {
                    ConnectedPoint::Dialer { address, .. } => address,
                    ConnectedPoint::Listener { send_back_addr, .. } => send_back_addr,
                };
                self.cleanup_address(peer_id.as_ref(), addr);
            }
            DialError::Transport(addrs) => {
                for (addr, _) in addrs {
                    self.cleanup_address(peer_id.as_ref(), addr);
                }
            }
            _ => {}
        };
        // remove failed contact
        if let Some(peer_id) = peer_id {
            self.remove_contact(&peer_id, format!("dial failure: {error}").as_str())
        } else {
            log::warn!("Unknown peer dial failure: {}", error)
        }
    }

    fn on_listen_failure(
        &mut self,
        local_addr: &Multiaddr,
        send_back_addr: &Multiaddr,
    ) {
        log::warn!(
            "Error accepting incoming connection from {} to our local address {}",
            send_back_addr,
            local_addr
        );
    }


    fn cleanup_address(&mut self, peer_id: Option<&PeerId>, addr: &Multiaddr) {
        // Notify those who waits for address dial
        if let Some(outs) = self.dialing.remove(addr) {
            for out in outs {
                out.send(None).ok();
            }
        }

        let _: Option<()> = try {
            let peer_id = peer_id?;
            let contact = self.contacts.get_mut(peer_id)?;

            contact.connected.remove(addr);
            contact.discovered.remove(addr);
            contact.dialing.remove(addr);
            if contact.dialing.is_empty() {
                let dial_promises = std::mem::take(&mut contact.dial_promises);
                for out in dial_promises {
                    out.send(false).ok();
                }
            }
            if contact.connected.is_empty() && contact.dialing.is_empty() {
                self.remove_contact(
                    peer_id,
                    "no more connected or dialed addresses after 'cleanup_address' call",
                );
            }
        };
    }
}

impl NetworkBehaviour for ConnectionPoolBehaviour {
    type ConnectionHandler = OneShotHandler<ProtocolConfig, HandlerMessage, HandlerMessage>;
    type OutEvent = ();

    fn new_handler(&mut self) -> Self::ConnectionHandler {
        self.protocol_config.clone().into()
    }

    // TODO: seems like there's no need in that method anymore IFF it is used only for dialing
    //       see https://github.com/libp2p/rust-libp2p/blob/master/swarm/CHANGELOG.md#0320-2021-11-16
    //       ACTION: remove this method. ALSO: remove `self.contacts`?
    fn addresses_of_peer(&mut self, peer_id: &PeerId) -> Vec<Multiaddr> {
        self.contacts
            .get(peer_id)
            .into_iter()
            .flat_map(|p| p.addresses().cloned())
            .collect()
    }

    fn on_swarm_event(&mut self, event: FromSwarm<'_, Self::ConnectionHandler>) {
        match event {
            FromSwarm::ConnectionEstablished(event) => {
                self.on_connection_established(
                    &event.peer_id,
                    event.endpoint,
                    &event.failed_addresses.to_vec(),
                );
            }
            FromSwarm::ConnectionClosed(event) => {
                self.on_connection_closed(
                    &event.peer_id,
                    event.endpoint,
                    event.remaining_established,
                );
            }
            FromSwarm::AddressChange(_) => {}
            FromSwarm::DialFailure(event) => {
                self.on_dial_failure(event.peer_id, event.error);
            }
            FromSwarm::ListenFailure(event) => {
                self.on_listen_failure(
                    event.local_addr,
                    event.send_back_addr,
                );
            }
            FromSwarm::NewListener(_) => {}
            FromSwarm::NewListenAddr(_) => {}
            FromSwarm::ExpiredListenAddr(_) => {}
            FromSwarm::ListenerError(_) => {}
            FromSwarm::ListenerClosed(_) => {}
            FromSwarm::NewExternalAddr(_) => {}
            FromSwarm::ExpiredExternalAddr(_) => {}
        }
    }

    fn on_connection_handler_event(&mut self, from: PeerId, _connection_id: ConnectionId, event: THandlerOutEvent<Self>) {
        match event {
            HandlerMessage::InParticle(particle) => {
                log::trace!(target: "network", "{}: received particle {} from {}; queue {}", self.peer_id, particle.id, from, self.queue.len());
                self.meter(|m| {
                    let particle_queue_size = i64::try_from(self.queue.len()).map(|x| x + 1);
                    match particle_queue_size {
                        Ok(particle_queue_size) => {
                            m.particle_queue_size.set(particle_queue_size);
                        }
                        Err(e) => log::warn!("Could not convert metric particle_queue_size {}", e),
                    }
                    m.received_particles.inc();
                    m.particle_sizes.observe(particle.data.len() as f64);
                });
                self.queue.push_back(particle);
                self.wake();
            }
            HandlerMessage::InboundUpgradeError(err) => log::warn!("UpgradeError: {:?}", err),
            HandlerMessage::Upgrade => {}
            HandlerMessage::OutParticle(..) => unreachable!("can't receive OutParticle"),
        }
    }

    fn poll(&mut self, cx: &mut Context<'_>, _params: &mut impl PollParameters) -> Poll<SwarmEventType> {
        self.waker = Some(cx.waker().clone());

        loop {
            // Check backpressure on the outlet
            let mut outlet = Pin::new(&mut self.outlet);
            match outlet.as_mut().poll_ready(cx) {
                Poll::Ready(Ok(_)) => {
                    // channel is ready to consume more particles, so send them
                    if let Some(particle) = self.queue.pop_front() {
                        let particle_id = particle.id.clone();
                        if let Err(err) = outlet.start_send(particle) {
                            log::error!("Failed to send particle to outlet: {}", err)
                        } else {
                            log::trace!(target: "execution", "Sent particle {} to execution", particle_id);
                        }
                    } else {
                        break;
                    }
                }
                Poll::Pending => {
                    // if channel is full, then keep particles in the queue
                    let len = self.queue.len();
                    if len > 30 {
                        log::warn!("Particle queue seems to have stalled; queue {}", len);
                    } else {
                        log::trace!(target: "network", "Connection pool outlet is pending; queue {}", len);
                    }
                    if self.outlet.is_closed() {
                        log::error!("Particle outlet closed");
                    }
                    break;
                }
                Poll::Ready(Err(err)) => {
                    log::warn!("ConnectionPool particle inlet has been dropped: {}", err);
                    break;
                }
            }
        }

        let particle_queue_size = i64::try_from(self.queue.len());
        match particle_queue_size {
            Ok(particle_queue_size) => {
                self.meter(|m| m.particle_queue_size.set(particle_queue_size));
            }
            Err(e) => log::warn!("Could not convert metric particle_queue_size {}", e),
        }

        while let Poll::Ready(Some(cmd)) = self.commands.poll_next_unpin(cx) {
            self.execute(cmd)
        }

        if let Some(event) = self.events.pop_front() {
            return Poll::Ready(event);
        }

        Poll::Pending
    }
}
