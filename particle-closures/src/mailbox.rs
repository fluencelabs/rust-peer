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

use crate::builtin_services_api::BuiltinServicesApi;

use json_utils::as_value;
use particle_dht::ResolveErrorKind;
use waiting_queues::WaitingQueues;

use futures::{channel::mpsc, StreamExt};
use libp2p::kad::record;
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    sync::mpsc as std_mpsc,
    task::{Context, Poll},
};

pub(super) type WaitResult = std_mpsc::Receiver<BuiltinCommandResult>;
pub(super) type WaitingVM = std_mpsc::Sender<BuiltinCommandResult>;
pub(super) type Inbox = mpsc::UnboundedReceiver<Command>;
pub(super) type Destination = mpsc::UnboundedSender<Command>;

#[derive(Debug)]
pub struct Command {
    pub outlet: WaitingVM,
    pub kind: BuiltinCommand,
}

#[derive(Debug, Hash, Clone, PartialEq, Eq)]
pub enum Key {
    DHT(libp2p::kad::record::Key),
}

#[derive(Debug, Clone)]
pub enum BuiltinCommand {
    DHTResolve(record::Key),
}

impl BuiltinCommand {
    pub fn key(&self) -> Key {
        match self {
            BuiltinCommand::DHTResolve(key) => Key::DHT(key.clone()),
        }
    }
}

#[derive(Debug, Clone)]
pub enum BuiltinCommandResult {
    DHTResolved(Result<Vec<Vec<u8>>, ResolveErrorKind>),
}

impl Into<Result<Value, Value>> for BuiltinCommandResult {
    fn into(self) -> Result<Value, Value> {
        match self {
            BuiltinCommandResult::DHTResolved(v) => v.map(|vs| json!(vs)).map_err(as_value),
        }
    }
}

#[derive(Debug)]
pub struct Mailbox {
    waiting: WaitingQueues<Key, Command>,
    inbox: Inbox,
    destination: Destination,
}

// Infrastructure
impl Mailbox {
    pub fn new() -> Self {
        let (destination, inbox) = mpsc::unbounded();
        Self {
            inbox,
            destination,
            waiting: <_>::default(),
        }
    }
}

impl Default for Mailbox {
    fn default() -> Self {
        Self::new()
    }
}

// VM API
impl Mailbox {
    fn get_destination(&self) -> Destination {
        self.destination.clone()
    }

    pub fn get_api(&self) -> BuiltinServicesApi {
        BuiltinServicesApi::new(self.get_destination())
    }
}

// Behaviour API
impl Mailbox {
    pub fn resolve_complete(
        &mut self,
        key: record::Key,
        value: Result<HashSet<Vec<u8>>, ResolveErrorKind>,
    ) {
        let value = value.map(|v| v.into_iter().collect());
        for cmd in self.waiting.remove(&Key::DHT(key)) {
            let result = BuiltinCommandResult::DHTResolved(value.clone());
            cmd.outlet.send(result.clone()).expect("resolve_complete")
        }
    }

    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BuiltinCommand> {
        match self.inbox.poll_next_unpin(cx) {
            Poll::Ready(Some(cmd)) => {
                let kind = cmd.kind.clone();
                self.waiting.enqueue(kind.key(), cmd);
                Poll::Ready(kind)
            }
            Poll::Ready(None) => unreachable!("destination couldn't be dropped"),
            Poll::Pending => Poll::Pending,
        }
    }
}
