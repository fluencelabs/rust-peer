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

use crate::error::ServiceError;
use crate::persistence::{persist_service, PersistedService};
use crate::Result;

use particle_modules::ModuleRepository;
use server_config::ServicesConfig;

use fluence_app_service::{AppService, AppServiceConfig, FaaSConfig, ModuleDescriptor};
use std::path::Path;

pub fn create_app_service(
    config: ServicesConfig,
    modules: &ModuleRepository,
    blueprint_id: String,
    service_id: String,
    aliases: Vec<String>,
    owner_id: String,
) -> Result<AppService> {
    try {
        let mut modules_config = modules.resolve_blueprint(&blueprint_id)?;
        modules_config
            .iter_mut()
            .for_each(|module| inject_vault(&config.particles_vault_dir, module));

        let modules = AppServiceConfig {
            service_base_dir: config.workdir,
            faas_config: FaaSConfig {
                modules_dir: Some(config.modules_dir),
                modules_config,
                default_modules_config: None,
            },
        };

        log::debug!("Creating service {}, envs: {:?}", service_id, config.envs);

        let service = AppService::new(modules, service_id.clone(), config.envs)
            .map_err(ServiceError::Engine)?;

        // Save created service to disk, so it is recreated on restart
        let persisted = PersistedService::new(service_id, blueprint_id, aliases, owner_id);
        persist_service(&config.services_dir, persisted)?;

        service
    }
}

/// Map `vault_dir` to `/tmp/vault` inside the service.
/// Particle File Vaults will be available as `/tmp/vault/$particle_id`
fn inject_vault(vault_dir: &Path, module: &mut ModuleDescriptor) {
    let wasi = &mut module.config.wasi;
    if let None = *wasi {
        *wasi = <_>::default();
    }

    let vault_dir = vault_dir.to_path_buf();
    let wasi = wasi.as_mut().unwrap();

    wasi.preopened_files.insert(vault_dir.clone());
    wasi.mapped_dirs.insert("/tmp/vault".into(), vault_dir);
}
