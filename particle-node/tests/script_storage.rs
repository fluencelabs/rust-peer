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

use test_utils::{enable_logs, make_swarms, read_args, timeout, ConnectedClient};

use fstrings::f;
use maplit::hashmap;
use serde_json::json;
use std::thread::sleep;
use std::time::Duration;

#[macro_use]
extern crate fstrings;

#[test]
fn stream_hello() {
    let swarms = make_swarms(1);

    let mut client = ConnectedClient::connect_to(swarms[0].1.clone()).expect("connect client");

    let script = f!(r#"
        (call "{client.peer_id}" ("op" "return") ["hello"])
    "#);

    client.send_particle(
        r#"
        (call relay ("script" "add") [script "0"])
        "#,
        hashmap! {
            "relay" => json!(client.node.to_string()),
            "script" => json!(script),
        },
    );

    for _ in 1..10 {
        let res = client.receive_args().into_iter().next().unwrap();
        assert_eq!(res, "hello");
    }
}

#[test]
fn remove_script() {
    let swarms = make_swarms(1);

    let mut client = ConnectedClient::connect_to(swarms[0].1.clone()).expect("connect client");

    let script = f!(r#"
        (call "{client.peer_id}" ("op" "return") ["hello"])
    "#);

    client.send_particle(
        r#"
        (seq
            (call relay ("script" "add") [script "0"] id)
            (call client ("op" "return") [id])
        )
        "#,
        hashmap! {
            "relay" => json!(client.node.to_string()),
            "client" => json!(client.peer_id.to_string()),
            "script" => json!(script),
        },
    );

    let script_id = client.receive_args().into_iter().next().unwrap();
    let remove_id = client.send_particle(
        r#"
        (seq
            (call relay ("script" "remove") [id] removed)
            (call client ("op" "return") [removed])
        )
        "#,
        hashmap! {
            "relay" => json!(client.node.to_string()),
            "client" => json!(client.peer_id.to_string()),
            "id" => json!(script_id),
        },
    );

    async_std::task::block_on(timeout(
        Duration::from_secs(5),
        async_std::task::spawn(async move {
            loop {
                let particle = client.receive();
                if particle.id == remove_id {
                    let removed = read_args(particle, &client.peer_id);
                    assert_eq!(removed, vec![serde_json::Value::Bool(true)]);
                    break;
                }
            }
        }),
    ))
    .expect("script wasn't deleted");
}

#[test]
/// Check that auto-particle can be delivered through network hops
fn script_routing() {
    let swarms = make_swarms(3);

    let mut client = ConnectedClient::connect_to(swarms[0].1.clone()).expect("connect client");

    let script = f!(r#"
        (seq
            (call "{client.node}" ("op" "identity") [])
            (call "{client.peer_id}" ("op" "return") ["hello"])
        )
    "#);

    client.send_particle(
        r#"
        (seq
            (call relay ("op" "identity") [])
            (call second ("script" "add") [script "0"] id)
        )
        "#,
        hashmap! {
            "relay" => json!(client.node.to_string()),
            "second" => json!(swarms[1].0.to_string()),
            "script" => json!(script),
        },
    );

    for _ in 1..10 {
        let res = client.receive_args().into_iter().next().unwrap();
        assert_eq!(res, "hello");
    }
}
