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

use async_std::task;
use faas_api::FunctionCall;
use fluence_faas::{FaaSError, FaaSInterface, FluenceFaaS, IValue, RawCoreModulesConfig};
use futures_util::future::BoxFuture;
use libp2p::core::connection::ConnectionId;
use libp2p::swarm::{
    protocols_handler::DummyProtocolsHandler, NetworkBehaviour, NetworkBehaviourAction,
    PollParameters,
};
use libp2p::PeerId;
use parity_multiaddr::Multiaddr;
use std::collections::HashMap;
use std::error::Error;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use uuid::Uuid;
use void::Void;

type Result<T> = std::result::Result<T, FaaSExecError>;
type FaasResult = Vec<IValue>;
type FutResult = (FluenceFaaS, FunctionCall, Result<CallResult>);
type Fut = BoxFuture<'static, FutResult>;

#[derive(Debug)]
pub enum FaaSExecError {
    NoSuchInstance(String),
    FaaS(FaaSError),
}

impl Error for FaaSExecError {}
impl From<FaaSError> for FaaSExecError {
    fn from(err: FaaSError) -> Self {
        FaaSExecError::FaaS(err)
    }
}

impl std::fmt::Display for FaaSExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FaaSExecError::NoSuchInstance(service_id) => {
                write!(f, "FaaS instance {} not found", service_id)
            }
            FaaSExecError::FaaS(err) => err.fmt(f),
        }
    }
}

#[derive(Debug, Clone)]
pub enum CallResult {
    FaaSCreated { service_id: String },
    Returned(FaasResult),
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum FaaSCall {
    /// Call to the FaaS instance specified by `service_id`
    Call {
        /// UUID of the FaaS instance
        service_id: String,
        /// Module to call function on
        module: String,
        /// Function name to call
        function: String,
        /// Arguments for the function
        arguments: Vec<IValue>,
        /// FunctionCall that caused this WasmCall, returned to caller as is
        call: FunctionCall,
    },
    Create {
        /// Context: list of modules to load
        module_names: Vec<String>,
        /// FunctionCall that caused this WasmCall, returned to caller as is
        call: FunctionCall,
    },
}

impl FaaSCall {
    pub fn is_create(&self) -> bool {
        match self {
            FaaSCall::Create { .. } => true,
            FaaSCall::Call { .. } => false,
        }
    }

    pub fn service_id(&self) -> Option<&str> {
        match self {
            FaaSCall::Call { service_id, .. } => Some(service_id),
            FaaSCall::Create { .. } => None,
        }
    }
}

pub struct FaaSBehaviour {
    faases: HashMap<String, FluenceFaaS>,
    calls: Vec<FaaSCall>,
    waker: Option<Waker>,
    futures: HashMap<String, Fut>,
    config: RawCoreModulesConfig,
}

impl FaaSBehaviour {
    #[allow(dead_code)]
    pub fn new(config: RawCoreModulesConfig) -> Self {
        Self {
            faases: <_>::default(),
            calls: <_>::default(),
            waker: <_>::default(),
            futures: <_>::default(),
            config,
        }
    }

    #[allow(dead_code)]
    pub fn execute(&mut self, call: FaaSCall) {
        self.calls.push(call);
        self.wake();
    }

    #[allow(dead_code)]
    pub fn get_interface(&self, service_id: &str) -> Result<FaaSInterface<'_>> {
        let faas = self
            .faases
            .get(service_id)
            .ok_or_else(|| FaaSExecError::NoSuchInstance(service_id.to_string()))?;

        Ok(faas.get_interface())
    }

    pub fn get_interfaces(&self) -> HashMap<&str, FaaSInterface<'_>> {
        self.faases
            .iter()
            .map(|(k, v)| (k.as_str(), v.get_interface()))
            .collect()
    }

    pub fn get_modules(&self) -> impl Iterator<Item = &str> {
        self.config.core_module.iter().map(|m| m.name.as_str())
    }

    fn create_faas(&self, module_names: Vec<String>) -> Result<(String, FluenceFaaS)> {
        let mut module_names = module_names.into_iter().collect();
        let uuid = Uuid::new_v4().to_string();
        let config = self.config.clone();

        let faas = FluenceFaaS::with_module_names(&mut module_names, config)?;
        Ok((uuid, faas))
    }

    /// Spawns tasks for calls execution and creates new FaaS-es until an error happens
    fn execute_calls<I>(
        &mut self,
        new_work: &mut I,
    ) -> std::result::Result<(), (FunctionCall, FaaSExecError)>
    where
        I: Iterator<Item = FaaSCall>,
    {
        new_work.try_fold((), |_, call| {
            match call {
                // Request to create FaaS instance with given module_names
                FaaSCall::Create { module_names, call } => {
                    let (service_id, faas) = self
                        .create_faas(module_names)
                        .map_err(|e| (call.clone(), e))?;

                    let result = CallResult::FaaSCreated {
                        // TODO: excess clone, data duplication:
                        //  service_id stored in futures as key and as value in FaaSCreated :(
                        service_id: service_id.clone(),
                    };
                    let result = (faas, call, Ok(result));

                    // Save completed future so result is returned on the next poll
                    let future = async_std::future::ready(result);
                    self.futures.insert(service_id, Box::pin(future));

                    self.wake();


                    Ok(())
                }
                // Request to call function on an existing FaaS instance
                #[rustfmt::skip]
                FaaSCall::Call { service_id, module, function, arguments, call } => {
                    // Take existing faas
                    let mut faas = self
                        .faases
                        .remove(&service_id)
                        .ok_or_else(|| (call.clone(), FaaSExecError::NoSuchInstance(service_id.clone())))?;
                    let waker = self.waker.clone();
                    // Spawn a task that will call wasm function
                    let future = task::spawn_blocking(move || {
                        let result = faas.call_module(&module, &function, &arguments);
                        let result = result.map(|r| CallResult::Returned(r)).map_err(|e| e.into());
                        Self::call_wake(waker);
                        (faas, call, result)
                    });
                    // Save future for the next poll
                    self.futures.insert(service_id, Box::pin(future));

                    Ok(())
                }
            }
        })
    }

    fn call_wake(waker: Option<Waker>) {
        if let Some(waker) = waker {
            waker.wake()
        }
    }

    fn wake(&self) {
        Self::call_wake(self.waker.clone())
    }
}

impl NetworkBehaviour for FaaSBehaviour {
    type ProtocolsHandler = DummyProtocolsHandler;
    type OutEvent = (FunctionCall, Result<CallResult>);

    fn new_handler(&mut self) -> Self::ProtocolsHandler {
        Default::default()
    }

    fn addresses_of_peer(&mut self, _: &PeerId) -> Vec<Multiaddr> {
        vec![]
    }

    fn inject_connected(&mut self, _: &PeerId) {}

    fn inject_disconnected(&mut self, _: &PeerId) {}

    fn inject_event(&mut self, _: PeerId, _: ConnectionId, _: Void) {}

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        _: &mut impl PollParameters,
    ) -> Poll<NetworkBehaviourAction<Void, Self::OutEvent>> {
        self.waker = Some(cx.waker().clone());

        // Check there is a completed call
        let mut result = None;
        // let mut futures = std::mem::replace(&mut self.futures, HashMap::new());
        for (key, fut) in self.futures.iter_mut() {
            let fut = Pin::new(fut);
            if let Poll::Ready(r) = fut.poll(cx) {
                // TODO: excess clone (possible to obtain via futures.remove_entry
                result = Some((key.clone(), r));
                break;
            }
        }
        // Remove completed future, reinsert faas, return result
        if let Some((service_id, result)) = result {
            self.futures.remove(&service_id);

            let (faas, call, result): FutResult = result;
            self.faases.insert(service_id, faas);

            return Poll::Ready(NetworkBehaviourAction::GenerateEvent((call, result)));
        }

        // Check if there's a work and a matching faas isn't busy
        let capacity = self.calls.capacity();
        let calls = std::mem::replace(&mut self.calls, Vec::with_capacity(capacity));
        let (new_work, busy): (Vec<_>, _) = calls.into_iter().partition(|call| {
            // return true if service is to be created, or there is no existing work for that service_id
            call.is_create() || !self.futures.contains_key(call.service_id().unwrap())
        });
        self.calls.extend(busy);

        // Execute calls on faases
        let mut new_work = new_work.into_iter();
        // Iterate until an error is "found"
        let err = self.execute_calls(&mut new_work);

        // If error happened during call execution
        if let Err((call, err)) = err {
            // Put left work back to the queue
            self.calls.extend(new_work);
            // Return the error
            return Poll::Ready(NetworkBehaviourAction::GenerateEvent((call, Err(err))));
        }

        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    static TEST_MODULE: &str = "./tests/artifacts/test_module_wit.wasi.wasm";

    use super::*;
    // use async_std::task;
    use fluence_faas::RawCoreModulesConfig;
    use futures::StreamExt;
    use futures::{executor::block_on, future::poll_fn};
    use libp2p::core::transport::dummy::{DummyStream, DummyTransport};
    use libp2p::mplex::Multiplex;
    use libp2p::{PeerId, Swarm};
    use std::path::PathBuf;

    fn wait_result(
        mut swarm: Swarm<FaaSBehaviour>,
    ) -> ((FunctionCall, Result<CallResult>), Swarm<FaaSBehaviour>) {
        block_on(async move {
            let result = poll_fn(|ctx| {
                loop {
                    match swarm.poll_next_unpin(ctx) {
                        Poll::Ready(Some(r)) => return Poll::Ready(r),
                        _ => break,
                    }
                }

                Poll::Pending
            })
            .await;

            (result, swarm)
        })
    }

    fn empty_call() -> FunctionCall {
        FunctionCall {
            uuid: "uuid".to_string(),
            target: None,
            reply_to: None,
            module: None,
            fname: None,
            arguments: Default::default(),
            name: None,
            sender: Default::default(),
        }
    }

    fn with_modules<P: Into<PathBuf>>(modules: Vec<(String, P)>) -> RawCoreModulesConfig {
        let mut tmp = std::env::temp_dir();
        tmp.push("wasm_modules/");
        std::fs::create_dir_all(&tmp).expect("create tmp dir");

        for (name, path) in modules {
            let path = path.into();
            std::fs::copy(&path, tmp.join(&name))
                .expect(format!("copy test module wasm {:?}", path).as_str());
        }

        let mut config: RawCoreModulesConfig = <_>::default();
        config.core_modules_dir = Some(tmp.to_string_lossy().into());

        config
    }

    fn make_swarm<P: Into<PathBuf>>(modules: Vec<(String, P)>) -> Swarm<FaaSBehaviour> {
        let config = with_modules(modules);
        let behaviour = FaaSBehaviour::new(config);
        let transport = DummyTransport::<(PeerId, Multiplex<DummyStream>)>::new();
        Swarm::new(transport, behaviour, PeerId::random())
    }

    fn create_faas(
        mut swarm: Swarm<FaaSBehaviour>,
        module_names: Vec<String>,
    ) -> (String, Swarm<FaaSBehaviour>) {
        swarm.execute(FaaSCall::Create {
            module_names,
            call: empty_call(),
        });

        let ((_, created), swarm) = wait_result(swarm);
        let service_id = match &created {
            Ok(CallResult::FaaSCreated { service_id }) => service_id.clone(),
            wrong => unreachable!("wrong result: {:?}", wrong),
        };

        (service_id, swarm)
    }

    fn call_faas(
        mut swarm: Swarm<FaaSBehaviour>,
        service_id: String,
        module: &str,
        function: &str,
        argument: Option<&str>,
    ) -> (FaasResult, Swarm<FaaSBehaviour>) {
        swarm.execute(FaaSCall::Call {
            service_id,
            module: module.to_string(),
            function: function.to_string(),
            arguments: argument
                .into_iter()
                .map(|s| IValue::String(s.to_string()))
                .collect(),
            call: empty_call(),
        });

        let ((_, returned), swarm) = wait_result(swarm);
        let returned = match returned {
            Ok(CallResult::Returned(r)) => r,
            wrong => panic!("{:#?}", wrong),
        };

        (returned, swarm)
    }

    #[test]
    fn call_single_faas() {
        let test_module = "test_module.wasm".to_string();
        let swarm = make_swarm(vec![(test_module.clone(), TEST_MODULE)]);
        let (service_id, swarm) = create_faas(swarm, vec![test_module.clone()]);

        let interface = swarm
            .get_interface(service_id.as_str())
            .expect("get interface");
        assert_eq!(1, interface.modules.len());
        assert_eq!(
            &test_module,
            interface.modules.into_iter().next().unwrap().0
        );

        let payload = "Hello";
        let (returned, _) = call_faas(
            swarm,
            service_id,
            test_module.as_str(),
            "greeting",
            Some(payload),
        );
        assert_eq!(returned, vec![IValue::String(payload.to_string())]);
    }

    #[test]
    fn call_multiple_faases() {
        let test_module = "test_module.wasm".to_string();
        let test_module2 = "test_module2.wasm".to_string();
        let modules = vec![
            (test_module.clone(), TEST_MODULE),
            (test_module2.clone(), TEST_MODULE),
        ];
        let swarm = make_swarm(modules);

        let (_, swarm) = create_faas(swarm, vec![test_module.clone()]);
        let (service_id2, swarm) =
            create_faas(swarm, vec![test_module.clone(), test_module2.clone()]);

        assert_eq!(
            2,
            swarm
                .get_interface(&service_id2)
                .expect("get interface")
                .modules
                .len()
        );

        let payload = "hello";
        let (returned, swarm) = call_faas(
            swarm,
            service_id2.clone(),
            test_module.as_str(),
            "greeting",
            Some(payload),
        );

        let (returned2, _swarm) = call_faas(
            swarm,
            service_id2,
            test_module2.as_str(),
            "greeting",
            Some(payload),
        );

        assert_eq!(returned, vec![IValue::String(payload.to_string())]);
        assert_eq!(returned2, returned);
    }
}
