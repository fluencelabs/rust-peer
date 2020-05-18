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

#![cfg(test)]
#![recursion_limit = "512"]
#![warn(missing_debug_implementations, rust_2018_idioms, missing_docs)]
#![deny(
    dead_code,
    nonstandard_style,
    unused_imports,
    unused_mut,
    unused_variables,
    unused_unsafe,
    unreachable_patterns
)]

mod utils;

use crate::utils::*;

use faas_api::{service, FunctionCall};
use libp2p::{identity::PublicKey::Ed25519, PeerId};
use serde_json::Value;
use trust_graph::{current_time, Certificate};
use uuid::Uuid;

use std::str::FromStr;
use std::thread::sleep;

#[test]
// Send calls between clients through relays
fn send_call() {
    let (sender, mut receiver) = ConnectedClient::make_clients().expect("connect clients");

    let uuid = Uuid::new_v4().to_string();
    let call = FunctionCall {
        uuid: uuid.clone(),
        target: Some(receiver.relay_address()),
        reply_to: Some(sender.relay_address()),
        name: None,
        arguments: Value::Null,
    };

    sender.send(call);
    let received = receiver.receive();
    assert_eq!(received.uuid, uuid);

    // Check there is no more messages
    let bad = receiver.maybe_receive();
    assert_eq!(
        bad,
        None,
        "received unexpected message {}, previous was {}",
        bad.as_ref().unwrap().uuid,
        received.uuid
    );
}

#[test]
// Provide service, and check that call reach it
fn call_service() {
    let service_id = "someserviceilike";
    let (mut provider, consumer) = ConnectedClient::make_clients().expect("connect clients");

    // Wait until Kademlia is ready // TODO: wait for event from behaviour instead?
    sleep(KAD_TIMEOUT);

    let provide = provide_call(service_id, provider.relay_address());
    provider.send(provide);

    let call_service = service_call(service_id, consumer.relay_address());
    consumer.send(call_service.clone());

    let to_provider = provider.receive();

    assert_eq!(
        call_service.uuid, to_provider.uuid,
        "Got: {:?}",
        to_provider
    );
    assert_eq!(
        to_provider.target,
        Some(provider.client_address().extend(service!(service_id)))
    );
}

#[test]
fn call_service_reply() {
    let service_id = "plzreply";
    let (mut provider, mut consumer) = ConnectedClient::make_clients().expect("connect clients");

    // Wait until Kademlia is ready // TODO: wait for event from behaviour instead?
    sleep(KAD_TIMEOUT);

    let provide = provide_call(service_id, provider.relay_address());
    provider.send(provide);

    let call_service = service_call(service_id, consumer.relay_address());
    consumer.send(call_service.clone());

    let to_provider = provider.receive();
    assert_eq!(to_provider.reply_to, Some(consumer.relay_address()));

    let reply = reply_call(to_provider.reply_to.unwrap());
    provider.send(reply.clone());

    let to_consumer = consumer.receive();
    assert_eq!(reply.uuid, to_consumer.uuid, "Got: {:?}", to_consumer);
    assert_eq!(to_consumer.target, Some(consumer.client_address()));
}

#[test]
// 1. Provide some service
// 2. Disconnect provider – service becomes unregistered
// 3. Check that calls to service fail
// 4. Provide same service again, via different provider
// 5. Check that calls to service succeed
fn provide_disconnect() {
    let service_id = "providedisconnect";

    let (mut provider, mut consumer) = ConnectedClient::make_clients().expect("connect clients");
    // Wait until Kademlia is ready // TODO: wait for event from behaviour instead?
    sleep(KAD_TIMEOUT);

    // Register service
    let provide = provide_call(service_id, provider.relay_address());
    provider.send(provide);
    // Check there was no error // TODO: maybe send reply from relay?
    let error = provider.maybe_receive();
    assert_eq!(error, None);

    // Disconnect provider, service should be deregistered
    provider.client.stop();

    // Send call to the service, should fail
    let mut call_service = service_call(service_id, consumer.relay_address());
    call_service.name = Some("Send call to the service, should fail".into());
    consumer.send(call_service.clone());
    let error = consumer.receive();
    assert!(error.uuid.starts_with("error_"));

    // Register the service once again
    // let bootstraps = vec![provider.node_address.clone(), consumer.node_address.clone()];
    let mut provider =
        ConnectedClient::connect_to(provider.node_address).expect("connect provider");
    let provide = provide_call(service_id, provider.relay_address());
    provider.send(provide);
    let error = provider.maybe_receive();
    assert_eq!(error, None);

    // Send call to the service once again, should succeed
    call_service.name = Some("Send call to the service , should succeed".into());
    consumer.send(call_service.clone());
    let to_provider = provider.receive();

    assert_eq!(call_service.uuid, to_provider.uuid);
    assert_eq!(
        to_provider.target,
        Some(provider.client_address().extend(service!(service_id)))
    );
}

#[test]
// Receive error when there's not enough nodes to store service in DHT
fn provide_error() {
    let mut provider = ConnectedClient::new().expect("connect client");
    let service_id = "failedservice";
    let provide = provide_call(service_id, provider.relay_address());
    provider.send(provide);
    let error = provider.receive();
    assert!(error.uuid.starts_with("error_"));
}

// TODO: test on invalid signature
// TODO: test on missing signature

#[test]
fn reconnect_provide() {
    let service_id = "popularservice";
    let swarms = make_swarms(5);
    sleep(KAD_TIMEOUT);
    let consumer = ConnectedClient::connect_to(swarms[1].1.clone()).expect("connect consumer");

    for _i in 1..20 {
        for swarm in swarms.iter() {
            let provider = ConnectedClient::connect_to(swarm.1.clone()).expect("connect provider");
            let provide_call = provide_call(service_id, provider.relay_address());
            provider.send(provide_call);
            sleep(SHORT_TIMEOUT);
        }
    }
    println!("after main cycle");

    sleep(SHORT_TIMEOUT);

    let mut provider = ConnectedClient::connect_to(swarms[0].1.clone()).expect("connect provider");
    let provide_call = provide_call(service_id, provider.relay_address());
    provider.send(provide_call);

    sleep(KAD_TIMEOUT);

    let call_service = service_call(service_id, consumer.relay_address());
    consumer.send(call_service.clone());

    let to_provider = provider.receive();
    assert_eq!(to_provider.uuid, call_service.uuid);
}

#[test]
fn get_certs() {
    let cert = get_cert();
    let first_key = cert.chain.first().unwrap().issued_for.clone();
    let last_key = cert.chain.last().unwrap().issued_for.clone();

    let trust = Trust {
        root_weights: vec![(first_key, 1)],
        certificates: vec![cert.clone()],
        cur_time: current_time(),
    };

    let swarm_count = 5;
    let swarms = make_swarms_with(swarm_count, |bs, maddr| {
        create_swarm(bs, maddr, Some(trust.clone()))
    });
    sleep(KAD_TIMEOUT);
    let mut consumer = ConnectedClient::connect_to(swarms[1].1.clone()).expect("connect consumer");
    let peer_id = PeerId::from(Ed25519(last_key));
    let call = certificates_call(peer_id, consumer.relay_address());
    consumer.send(call.clone());

    // If count is small, all nodes should fit in neighborhood, and all of them should reply
    for _ in 0..swarm_count {
        let reply = consumer.receive();
        assert_eq!(reply.arguments["msg_id"], call.arguments["msg_id"]);
        let reply_certs = &reply.arguments["certificates"][0]
            .as_str()
            .expect("get str cert");
        let reply_certs = Certificate::from_str(reply_certs).expect("deserialize cert");

        assert_eq!(reply_certs, cert);
    }
}

// TODO: test on add_certs error
// TODO: test on get_certs error

#[test]
fn add_certs() {
    enable_logs();

    let cert = get_cert();
    let first_key = cert.chain.first().unwrap().issued_for.clone();
    let last_key = cert.chain.last().unwrap().issued_for.clone();

    let trust = Trust {
        root_weights: vec![(first_key, 1)],
        certificates: vec![],
        cur_time: current_time(),
    };

    let swarm_count = 5;
    let swarms = make_swarms_with(swarm_count, |bs, maddr| {
        create_swarm(bs, maddr, Some(trust.clone()))
    });
    sleep(KAD_TIMEOUT);

    let mut registrar = ConnectedClient::connect_to(swarms[1].1.clone()).expect("connect consumer");
    let peer_id = PeerId::from(Ed25519(last_key));
    let call = add_certificates_call(peer_id, registrar.relay_address(), vec![cert]);
    registrar.send(call.clone());

    // If count is small, all nodes should fit in neighborhood, and all of them should reply
    for _ in 0..swarm_count {
        let reply = registrar.receive();
        assert_eq!(reply.arguments["msg_id"], call.arguments["msg_id"]);
    }
}
