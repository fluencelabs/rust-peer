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

mod server;
mod behaviour {
    mod bootstrapper;
    mod identify;
    mod server_behaviour;

    pub use server_behaviour::ServerBehaviour;
}

pub mod config {
    mod app_services;
    mod args;
    mod defaults;
    mod fluence_config;
    mod keys;

    pub mod certificates;

    pub use self::args::create_args;
    pub use self::fluence_config::load_config;
    pub use self::fluence_config::FluenceConfig;
    pub use self::fluence_config::ServerConfig;
    pub use app_services::AppServicesConfig;
}

pub use behaviour::ServerBehaviour;
pub use server::Server;
