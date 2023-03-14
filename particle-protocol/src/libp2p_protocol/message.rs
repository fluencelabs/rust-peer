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

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::Particle;

#[derive(Debug, Default)]
pub enum SendStatus {
    Ok,
    TimedOut {
        after: Duration,
        error: std::io::Error,
    },
    ProtocolError(String),
    NotConnected,
    #[default]
    ConnectionPoolDied,
}

#[derive(Debug, Default)]
pub enum CompletionChannel {
    #[default]
    Ignore,
    Oneshot(oneshot::Sender<SendStatus>),
}

impl CompletionChannel {
    pub fn outlet(self) -> Option<oneshot::Sender<SendStatus>> {
        match self {
            CompletionChannel::Ignore => None,
            CompletionChannel::Oneshot(outlet) => Some(outlet),
        }
    }
}

#[derive(Debug)]
pub enum HandlerMessage {
    /// Particle being sent to remote peer. Contains a channel to signal write completion.
    /// Send-only, can't be received.
    OutParticle(Particle, CompletionChannel),
    /// Particle being received from a remote peer.
    /// Receive-only, can't be sent.
    InParticle(Particle),
    /// Error while receiving a message
    InboundUpgradeError(serde_json::Value),
    /// Dummy plug. Generated by the `OneshotHandler` when Inbound or Outbound Upgrade happened.
    Upgrade,
}

impl HandlerMessage {
    pub fn into_protocol_message(self) -> (ProtocolMessage, Option<oneshot::Sender<SendStatus>>) {
        match self {
            HandlerMessage::OutParticle(particle, channel) => {
                (ProtocolMessage::Particle(particle), channel.outlet())
            }
            HandlerMessage::InboundUpgradeError(err) => {
                (ProtocolMessage::InboundUpgradeError(err), None)
            }
            HandlerMessage::Upgrade => (ProtocolMessage::Upgrade, None),
            HandlerMessage::InParticle(_) => {
                unreachable!("InParticle is never sent, only received")
            }
        }
    }
}

// Required by OneShotHandler in inject_fully_negotiated_outbound. And that's because
// <ProtocolMessage as UpgradeOutbound>::Output is (), and OneshotHandler requires it to be
// convertible to OneshotHandler::TEvent which is a ProtocolMessage
impl From<()> for HandlerMessage {
    fn from(_: ()) -> HandlerMessage {
        HandlerMessage::Upgrade
    }
}
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "action")]
pub enum ProtocolMessage {
    Particle(Particle),
    /// Error while receiving a message
    InboundUpgradeError(serde_json::Value),
    // TODO: is it needed?
    Upgrade,
}

impl std::fmt::Display for ProtocolMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolMessage::Particle(particle) => particle.fmt(f),
            ProtocolMessage::InboundUpgradeError(error) => {
                write!(f, "InboundUpgradeError {error}")
            }
            ProtocolMessage::Upgrade => write!(f, "Upgrade"),
        }
    }
}

impl From<ProtocolMessage> for HandlerMessage {
    fn from(msg: ProtocolMessage) -> HandlerMessage {
        match msg {
            ProtocolMessage::Particle(p) => HandlerMessage::InParticle(p),
            ProtocolMessage::InboundUpgradeError(err) => HandlerMessage::InboundUpgradeError(err),
            ProtocolMessage::Upgrade => HandlerMessage::Upgrade,
        }
    }
}
