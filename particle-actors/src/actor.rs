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
#![allow(dead_code)]

use crate::actor::VmState::{Executing, Idle};
use crate::config::ActorConfig;
use crate::invoke::parse_invoke_result;
use async_std::pin::Pin;
use async_std::task;
use fluence_app_service::{AppService, AppServiceError, RawModulesConfig};
use futures::future::BoxFuture;
use futures::Future;
use libp2p::PeerId;
use particle_protocol::Particle;
use serde_json::json;
use std::collections::VecDeque;
use std::mem;
use std::path::PathBuf;
use std::task::{Context, Poll, Waker};

pub(super) type Fut = BoxFuture<'static, FutResult>;

pub struct FutResult {
    vm: AppService,
    effects: Vec<ActorEvent>,
}

pub enum ActorEvent {
    Forward { particle: Particle, target: PeerId },
}

enum VmState {
    Idle(AppService),
    Executing(Fut),
    Polling,
}

impl std::fmt::Display for VmState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            Idle(_) => "idle",
            Executing(_) => "executing",
            VmState::Polling => "polling",
        };
        write!(f, "{}", str)
    }
}

pub struct Actor {
    vm: VmState,
    particle: Particle,
    mailbox: VecDeque<Particle>,
    waker: Option<Waker>,
}

impl Actor {
    pub fn new(config: ActorConfig, particle: Particle) -> Result<Self, AppServiceError> {
        let vm = Self::create_vm(config, particle.id.clone(), particle.init_peer_id.clone())?;
        let mut this = Self {
            vm: Idle(vm),
            particle: particle.clone(),
            mailbox: <_>::default(),
            waker: <_>::default(),
        };

        this.ingest(particle);

        Ok(this)
    }

    pub fn particle(&self) -> &Particle {
        &self.particle
    }

    pub fn ingest(&mut self, particle: Particle) {
        self.mailbox.push_back(particle);
        self.wake();
    }

    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Vec<ActorEvent>> {
        let waker = cx.waker().clone();
        self.waker = Some(waker.clone());

        let vm = mem::replace(&mut self.vm, VmState::Polling);
        let execute = |vm| self.execute_next(vm, waker);
        let (state, effect) = match vm {
            Idle(vm) => (execute(vm), Poll::Pending),
            VmState::Executing(mut fut) => {
                if let Poll::Ready(FutResult { vm, effects }) = Pin::new(&mut fut).poll(cx) {
                    (execute(vm), Poll::Ready(effects))
                } else {
                    (VmState::Executing(fut), Poll::Pending)
                }
            }
            VmState::Polling => unreachable!("polling race"),
        };

        self.vm = state;
        return effect;
    }

    // TODO: check resolve works fine with new providers
    // TODO: remove provider on client disconnect

    fn create_vm(
        config: ActorConfig,
        particle_id: String,
        owner_id: PeerId,
    ) -> Result<AppService, AppServiceError> {
        let to_string =
            |path: &PathBuf| -> Option<_> { path.to_string_lossy().into_owned().into() };

        let modules = RawModulesConfig {
            modules_dir: to_string(&config.modules_dir),
            service_base_dir: to_string(&config.workdir),
            module: vec![config.stepper_config],
            default: None,
        };

        let mut envs = config.envs;
        envs.push(format!("owner_id={}", owner_id));
        /*
        if let Some(owner_pk) = owner_pk {
            envs.push(format!("owner_pk={}", owner_pk));
        };
        */
        log::info!("Creating service {}, envs: {:?}", particle_id, envs);

        AppService::new(modules, &particle_id, envs)

        // TODO: Save created service to disk, so it is recreated on restart
        // Self::persist_service(&config.services_dir, &service_id, &blueprint_id)?;
    }

    fn execute_next(&mut self, vm: AppService, waker: Waker) -> VmState {
        match self.mailbox.pop_front() {
            Some(p) => Executing(Self::execute(p, vm, waker)),
            None => Idle(vm),
        }
    }

    fn execute(particle: Particle, mut vm: AppService, waker: Waker) -> Fut {
        log::debug!("Scheduling particle for execution {:?}", particle.id);
        Box::pin(task::spawn_blocking(move || {
            log::info!("Executing particle {:?}", particle.id);

            let result = vm.call(
                "aquamarine",
                "invoke",
                json!({
                    "init_user_id": particle.init_peer_id.to_string(),
                    "aqua": particle.script,
                    "data": particle.data.to_string()
                }),
                <_>::default(),
            );

            log::debug!("Executed particle {:?}, parsing", particle.id);

            let effects = match parse_invoke_result(result) {
                Ok((data, targets)) => {
                    let mut particle = particle;
                    particle.data = data;
                    targets
                        .into_iter()
                        .map(|target| ActorEvent::Forward {
                            particle: particle.clone(),
                            target,
                        })
                        .collect::<Vec<_>>()
                }
                Err(err) => {
                    let mut particle = particle;
                    let error = format!("{:?}", err);
                    if let Some(map) = particle.data.as_object_mut() {
                        map.insert("error".to_string(), json!(error));
                    } else {
                        particle.data = json!({"error": error, "data": particle.data})
                    }
                    // Return error to the init peer id
                    vec![ActorEvent::Forward {
                        target: particle.init_peer_id.clone(),
                        particle,
                    }]
                }
            };

            log::debug!("Parsed result on particle");

            waker.wake();

            FutResult { vm, effects }
        }))
    }

    fn wake(&self) {
        if let Some(waker) = &self.waker {
            waker.wake_by_ref();
        }
    }
}
