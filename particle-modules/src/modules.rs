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

use crate::dependency::Dependency;
use crate::error::ModuleError::{
    BlueprintNotFound, EmptyDependenciesList, FacadeShouldBeHash, InvalidModuleName,
    ReadModuleInterfaceError,
};
use crate::error::Result;
use crate::file_names::{extract_module_file_name, is_module_wasm};
use crate::file_names::{module_config_name_hash, module_file_name_hash};
use crate::files::{load_config_by_path, load_module_by_path};
use crate::Hash;
use crate::{file_names, files, load_module_descriptor, Blueprint};

use fluence_app_service::{ModuleDescriptor, TomlFaaSNamedModuleConfig};
use host_closure::JError;
use marine_it_parser::module_interface;

use eyre::WrapErr;
use fstrings::f;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JValue};
use std::{collections::HashMap, iter, path::Path, path::PathBuf, sync::Arc};

type ModuleName = String;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AddBlueprint {
    pub name: String,
    pub dependencies: Vec<Dependency>,
}

#[derive(Clone)]
pub struct ModuleRepository {
    modules_dir: PathBuf,
    blueprints_dir: PathBuf,
    /// Map of module_config.name to blake3::hash(module bytes)
    modules_by_name: Arc<Mutex<HashMap<ModuleName, Hash>>>,
    module_interface_cache: Arc<RwLock<HashMap<Hash, JValue>>>,
    blueprints: Arc<RwLock<HashMap<String, Blueprint>>>,
}

impl ModuleRepository {
    pub fn new(modules_dir: &Path, blueprints_dir: &Path) -> Self {
        let modules_by_name: HashMap<_, _> = files::list_files(&modules_dir)
            .into_iter()
            .flatten()
            .filter(|path| is_module_wasm(&path))
            .filter_map(|path| {
                let name_hash: Result<_> = try {
                    let module = load_module_by_path(&path)?;
                    let hash = Hash::hash(&module);

                    Self::maybe_migrate_module(&path, &hash, &modules_dir);

                    let module = load_module_descriptor(&modules_dir, &hash)?;
                    (module.import_name, hash)
                };

                match name_hash {
                    Ok(name_hash) => Some(name_hash),
                    Err(err) => {
                        log::warn!("Error loading module list: {:?}", err);
                        None
                    }
                }
            })
            .collect();

        let modules_by_name = Arc::new(Mutex::new(modules_by_name));

        let blueprints = Self::load_blueprints(blueprints_dir);
        let blueprints_cache = Arc::new(RwLock::new(blueprints));

        Self {
            modules_by_name,
            modules_dir: modules_dir.to_path_buf(),
            blueprints_dir: blueprints_dir.to_path_buf(),
            module_interface_cache: <_>::default(),
            blueprints: blueprints_cache,
        }
    }

    /// check that module file name is equal to module hash
    /// if not, rename module and config files
    fn maybe_migrate_module(path: &Path, hash: &Hash, modules_dir: &Path) {
        use eyre::eyre;

        let migrated: eyre::Result<_> = try {
            let file_name = extract_module_file_name(&path).ok_or_else(|| eyre!("no file name"))?;
            if file_name != hash.to_hex().as_ref() {
                let new_name = module_file_name_hash(hash);
                log::debug!(target: "migration", "renaming module {}.wasm to {}", file_name, new_name);
                std::fs::rename(&path, modules_dir.join(module_file_name_hash(hash)))?;
                let new_name = module_config_name_hash(hash);
                let config = path.with_file_name(format!("{}_config.toml", file_name));
                log::debug!(target: "migration", "renaming config {:?} to {}", config.file_name().unwrap(), new_name);
                std::fs::rename(&config, modules_dir.join(new_name))?;
            }
        };

        if let Err(e) = migrated {
            log::warn!("Module {:?} migration failed: {:?}", path, e);
        }
    }

    /// Adds a module to the filesystem, overwriting existing module.
    pub fn add_module(&self, module: String, config: TomlFaaSNamedModuleConfig) -> Result<String> {
        let module = base64::decode(&module)?;
        let hash = Hash::hash(&module);

        let config = files::add_module(&self.modules_dir, &hash, &module, config)?;

        let module_hash = hash.to_hex().as_ref().to_owned();
        self.modules_by_name.lock().insert(config.name, hash);

        Ok(module_hash)
    }

    /// Saves new blueprint to disk
    pub fn add_blueprint(&self, blueprint: AddBlueprint) -> Result<String> {
        // resolve dependencies by name to hashes, if any
        let mut dependencies: Vec<Hash> = blueprint
            .dependencies
            .into_iter()
            .map(|module| Ok(resolve_hash(&self.modules_by_name, module)?))
            .collect::<Result<_>>()?;

        let blueprint_name = blueprint.name.clone();
        let facade = dependencies
            .pop()
            .ok_or_else(|| EmptyDependenciesList { id: blueprint_name })?;

        let hash = hash_dependencies(facade.clone(), dependencies.clone()).to_hex();

        let blueprint = Blueprint {
            id: hash.as_ref().to_string(),
            dependencies: dependencies
                .into_iter()
                .map(|h| Dependency::Hash(h))
                .chain(iter::once(Dependency::Hash(facade)))
                .collect(),
            name: blueprint.name,
        };
        files::add_blueprint(&self.blueprints_dir, &blueprint)?;

        self.blueprints
            .write()
            .insert(blueprint.id.clone(), blueprint.clone());

        Ok(blueprint.id)
    }

    pub fn list_modules(&self) -> std::result::Result<JValue, JError> {
        // TODO: refactor errors to enums
        let modules = files::list_files(&self.modules_dir)
            .into_iter()
            .flatten()
            .filter_map(|path| {
                let hash = extract_module_file_name(&path)?;
                let result: eyre::Result<_> = try {
                    let hash = Hash::from_hex(hash).wrap_err(f!("invalid module name {path:?}"))?;
                    let config = self.modules_dir.join(module_config_name_hash(&hash));
                    let config = load_config_by_path(&config).wrap_err(f!("load config ${config:?}"))?;

                    (hash, config)
                };

                let result = match result {
                    Ok((hash, config)) => json!({
                        "name": config.name,
                        "hash": hash.to_hex().as_ref(),
                        "config": config.config,
                    }),
                    Err(err) => {
                        log::warn!("list_modules error: {:?}", err);
                        json!({
                            "invalid_file_name": hash,
                            "error": format!("{:?}", err).split("Stack backtrace:").next().unwrap_or_default(),
                        })
                    }
                };

                Some(result)
            })
            .collect();

        Ok(modules)
    }

    pub fn get_facade_interface(&self, id: &str) -> Result<JValue> {
        let blueprints = self.blueprints.clone();

        let bp = {
            let lock = blueprints.read();
            lock.get(id).cloned()
        };

        match bp {
            None => Err(BlueprintNotFound { id: id.to_string() }),
            Some(bp) => {
                let dep = bp
                    .get_facade_module()
                    .ok_or(EmptyDependenciesList { id: id.to_string() })?;

                let hash = match dep {
                    Dependency::Hash(hash) => hash,
                    Dependency::Name(_) => return Err(FacadeShouldBeHash { id: id.to_string() }),
                };

                self.get_interface_by_hash(&hash)
            }
        }
    }

    pub fn get_interface_by_hash(&self, hash: &Hash) -> Result<JValue> {
        let cache: Arc<RwLock<HashMap<Hash, JValue>>> = self.module_interface_cache.clone();

        get_interface_by_hash(&self.modules_dir, cache, hash)
    }

    pub fn get_interface(&self, hex_hash: &str) -> std::result::Result<JValue, JError> {
        // TODO: refactor errors to ModuleErrors enum
        let interface: eyre::Result<_> = try {
            let hash = Hash::from_hex(hex_hash)?;

            get_interface_by_hash(
                &self.modules_dir,
                self.module_interface_cache.clone(),
                &hash,
            )?
        };

        interface.map_err(|err| {
            JError::new(
                format!("{:?}", err)
                    // TODO: send patch to eyre so it can be done through their API
                    // Remove backtrace from the response
                    .split("Stack backtrace:")
                    .next()
                    .unwrap_or_default(),
            )
        })
    }

    fn load_blueprints(blueprints_dir: &Path) -> HashMap<String, Blueprint> {
        let blueprints: Vec<Blueprint> = files::list_files(blueprints_dir)
            .into_iter()
            .flatten()
            .filter_map(|path| {
                // Check if file name matches blueprint schema
                let fname = path.file_name()?.to_str()?;
                if !file_names::is_blueprint(fname) {
                    return None;
                }

                let blueprint: eyre::Result<_> = try {
                    // Read & deserialize TOML
                    let bytes = std::fs::read(&path)?;
                    let blueprint: Blueprint = toml::from_slice(&bytes)?;
                    blueprint
                };

                match blueprint {
                    Ok(blueprint) => Some(blueprint),
                    Err(err) => {
                        log::warn!("load_blueprints error on file {}: {:?}", fname, err);
                        None
                    }
                }
            })
            .collect();

        let mut bp_map = HashMap::new();
        for bp in blueprints.iter() {
            bp_map.insert(bp.id.clone(), bp.clone());
        }

        bp_map
    }

    fn get_blueprint_from_cache(&self, id: &str) -> Result<Blueprint> {
        let blueprints = self.blueprints.clone();
        let blueprints = blueprints.read();
        blueprints
            .get(id)
            .cloned()
            .ok_or(BlueprintNotFound { id: id.to_string() })
    }

    /// Get available blueprints
    pub fn get_blueprints(&self) -> Vec<Blueprint> {
        self.blueprints.read().values().cloned().collect()
    }

    pub fn resolve_blueprint(&self, blueprint_id: &str) -> Result<Vec<ModuleDescriptor>> {
        let blueprint = self.get_blueprint_from_cache(blueprint_id)?;

        // Load all module descriptors
        let module_descriptors: Vec<_> = blueprint
            .dependencies
            .into_iter()
            .map(|module| {
                let hash = resolve_hash(&self.modules_by_name, module)?;
                let config = load_module_descriptor(&self.modules_dir, &hash)?;
                Ok(config)
            })
            .collect::<Result<_>>()?;

        Ok(module_descriptors)
    }
}

fn get_interface_by_hash(
    modules_dir: &Path,
    cache: Arc<RwLock<HashMap<Hash, JValue>>>,
    hash: &Hash,
) -> Result<JValue> {
    let interface_cache_opt = {
        let lock = cache.read();
        lock.get(hash).cloned()
    };

    let interface = match interface_cache_opt {
        Some(interface) => interface,
        None => {
            let path = modules_dir.join(module_file_name_hash(hash));
            let interface =
                module_interface(&path).map_err(|err| ReadModuleInterfaceError { path, err })?;
            let json = json!(interface);
            json
        }
    };

    cache.write().insert(hash.clone(), interface.clone());

    Ok(interface)
}

fn resolve_hash(
    modules: &Arc<Mutex<HashMap<ModuleName, Hash>>>,
    module: Dependency,
) -> Result<Hash> {
    match module {
        Dependency::Hash(hash) => Ok(hash),
        Dependency::Name(name) => {
            // resolve module hash by name
            let map = modules.lock();
            let hash = map.get(&name).cloned();
            hash.ok_or_else(|| InvalidModuleName(name.clone()))
        }
    }
}

pub fn hash_dependencies(facade: Hash, mut deps: Vec<Hash>) -> Hash {
    let mut hasher = blake3::Hasher::new();
    deps.sort_by(|a, b| a.as_bytes().cmp(&b.as_bytes()));

    for d in deps.iter().chain(iter::once(&facade)) {
        hasher.update(d.as_bytes());
    }

    let hash = hasher.finalize();
    let bytes = hash.as_bytes();
    Hash::from(*bytes)
}

#[cfg(test)]
mod tests {
    use fluence_app_service::{TomlFaaSModuleConfig, TomlFaaSNamedModuleConfig};
    use services_utils::load_module;
    use tempdir::TempDir;
    use test_utils::{add_bp, add_module, Dependency, Hash, ModuleRepository};

    #[test]
    fn test_add_blueprint() {
        let module_dir = TempDir::new("test").unwrap();
        let bp_dir = TempDir::new("test").unwrap();
        let repo = ModuleRepository::new(module_dir.path(), bp_dir.path());

        let dep1 = Dependency::Hash(Hash::hash(&[1, 2, 3]));
        let dep2 = Dependency::Hash(Hash::hash(&[3, 2, 1]));

        let name1 = "bp1".to_string();
        let resp1 = add_bp(&repo, name1.clone(), vec![dep1.clone(), dep2.clone()]).unwrap();
        let bps1 = repo.get_blueprints();
        assert_eq!(bps1.len(), 1);
        let bp1 = bps1.get(0).unwrap();
        assert_eq!(bp1.name, name1);

        let name2 = "bp2".to_string();
        let resp2 = add_bp(&repo, "bp2".to_string(), vec![dep1, dep2]).unwrap();
        let bps2 = repo.get_blueprints();
        assert_eq!(bps2.len(), 1);
        let bp2 = bps2.get(0).unwrap();
        assert_eq!(bp2.name, name2);

        assert_eq!(resp1, resp2);
        assert_eq!(bp1.id, bp2.id);
    }

    #[test]
    fn test_add_module_get_interface() {
        let module_dir = TempDir::new("test").unwrap();
        let bp_dir = TempDir::new("test2").unwrap();
        let repo = ModuleRepository::new(module_dir.path(), bp_dir.path());

        let module = load_module("../particle-node/tests/tetraplets/artifacts", "tetraplets");

        let config: TomlFaaSNamedModuleConfig = TomlFaaSNamedModuleConfig {
            name: "tetra".to_string(),
            file_name: None,
            config: TomlFaaSModuleConfig {
                mem_pages_count: None,
                logger_enabled: None,
                wasi: None,
                mounted_binaries: None,
                logging_mask: None,
            },
        };

        let hash = add_module(&repo, base64::encode(module), config).unwrap();

        let result = repo.get_interface(&hash);
        assert!(result.is_ok())
    }

    #[test]
    fn test_hash_dependency() {
        use super::hash_dependencies;
        use crate::modules::Hash;

        let dep1 = Hash::hash(&[1, 2, 3]);
        let dep2 = Hash::hash(&[2, 1, 3]);
        let dep3 = Hash::hash(&[3, 2, 1]);

        let hash1 = hash_dependencies(dep3.clone(), vec![dep1.clone(), dep2.clone()]);
        let hash2 = hash_dependencies(dep3.clone(), vec![dep2.clone(), dep1.clone()]);
        let hash3 = hash_dependencies(dep1.clone(), vec![dep2.clone(), dep3.clone()]);
        assert_eq!(hash1.to_string(), hash2.to_string());
        assert_ne!(hash2.to_string(), hash3.to_string());
    }
}
