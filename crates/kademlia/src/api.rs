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

use crate::error::{KademliaError, Result};
use crate::Kademlia;

use fluence_libp2p::generate_swarm_event_type;
use fluence_libp2p::types::{Inlet, OneshotInlet, OneshotOutlet, Outlet};

use futures::channel::mpsc::unbounded;
use futures::channel::oneshot;
use futures::future::BoxFuture;
use futures::{FutureExt, SinkExt, StreamExt, TryFutureExt};
use libp2p::core::Multiaddr;
use libp2p::swarm::NetworkBehaviourEventProcess;
use libp2p::PeerId;
use multihash::Multihash;
use std::convert::identity;

type Future<T> = BoxFuture<'static, T>;

pub trait KademliaApi {
    fn bootstrap(&self) -> Future<Result<()>>;
    fn local_lookup(&self, peer: PeerId) -> Future<Result<Vec<Multiaddr>>>;
    fn discover_peer(&self, peer: PeerId) -> Future<Result<(PeerId, Vec<Multiaddr>)>>;
    fn neighborhood(&self, key: Multihash) -> Future<Result<Vec<PeerId>>>;
}

#[derive(Debug)]
enum Command {
    LocalLookup {
        peer: PeerId,
        out: OneshotOutlet<Vec<Multiaddr>>,
    },
    Bootstrap {
        out: OneshotOutlet<Result<()>>,
    },
    DiscoverPeer {
        peer: PeerId,
        // TODO: Why returning PeerId back?
        out: OneshotOutlet<Result<(PeerId, Vec<Multiaddr>)>>,
    },
    Neighborhood {
        key: Multihash,
        out: OneshotOutlet<Result<Vec<PeerId>>>,
    },
}

pub type SwarmEventType = generate_swarm_event_type!(KademliaApiInlet);

#[derive(::libp2p::NetworkBehaviour)]
#[behaviour(poll_method = "custom_poll")]
pub struct KademliaApiInlet {
    #[behaviour(ignore)]
    inlet: Inlet<Command>,
    kademlia: Kademlia,
}

impl KademliaApiInlet {
    pub fn new(kademlia: Kademlia) -> (KademliaApiOutlet, Self) {
        let (outlet, inlet) = unbounded();
        let outlet = KademliaApiOutlet { outlet };
        (outlet, Self { inlet, kademlia })
    }

    fn execute(&mut self, cmd: Command) {
        match cmd {
            Command::Bootstrap { out } => self.kademlia.bootstrap(out),
            Command::LocalLookup { peer, out } => self.kademlia.local_lookup(&peer, out),
            Command::DiscoverPeer { peer, out } => self.kademlia.discover_peer(peer, out),
            Command::Neighborhood { key, out } => self.kademlia.neighborhood(key, out),
        }
    }

    fn custom_poll(
        &mut self,
        cx: &mut std::task::Context,
        _: &mut impl libp2p::swarm::PollParameters,
    ) -> std::task::Poll<SwarmEventType> {
        use std::task::Poll;

        while let Poll::Ready(Some(cmd)) = self.inlet.poll_next_unpin(cx) {
            self.execute(cmd)
        }

        Poll::Pending
    }
}

impl NetworkBehaviourEventProcess<()> for KademliaApiInlet {
    fn inject_event(&mut self, _: ()) {}
}

impl From<Kademlia> for (KademliaApiOutlet, KademliaApiInlet) {
    fn from(kademlia: Kademlia) -> Self {
        KademliaApiInlet::new(kademlia)
    }
}

#[derive(Clone)]
pub struct KademliaApiOutlet {
    outlet: Outlet<Command>,
}

impl KademliaApiOutlet {
    fn execute<R, F>(&self, cmd: F) -> Future<Result<R>>
    where
        R: Send + Sync + 'static,
        F: FnOnce(OneshotOutlet<Result<R>>) -> Command,
    {
        let (out, inlet) = oneshot::channel();
        if let Err(_) = self.outlet.unbounded_send(cmd(out)) {
            return futures::future::err(KademliaError::Cancelled).boxed();
        }
        inlet
            .map(|r| r.map_err(|_| KademliaError::Cancelled).and_then(identity))
            .boxed()
    }
}

impl KademliaApi for KademliaApiOutlet {
    fn local_lookup(&self, peer: PeerId) -> Future<Result<Vec<Multiaddr>>> {
        let (out, inlet) = oneshot::channel();
        self.outlet
            .unbounded_send(Command::LocalLookup { peer, out })
            .expect("kademlia api died");

        inlet.map_err(|err| KademliaError::Cancelled).boxed()
    }

    fn bootstrap(&self) -> Future<Result<()>> {
        self.execute(|out| Command::Bootstrap { out })
    }

    fn discover_peer(&self, peer: PeerId) -> Future<Result<(PeerId, Vec<Multiaddr>)>> {
        self.execute(|out| Command::DiscoverPeer { peer, out })
    }

    fn neighborhood(&self, key: Multihash) -> Future<Result<Vec<PeerId>>> {
        self.execute(|out| Command::Neighborhood { key, out })
    }
}
