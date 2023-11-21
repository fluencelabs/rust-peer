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

use fs_utils::{create_dir, list_files};

use crate::error::KeyManagerError;
use crate::error::KeyManagerError::{
    CannotExtractRSASecretKey, CreateKeypairsDir, DeserializePersistedKeypair,
    ReadPersistedKeypair, SerializePersistedKeypair, WriteErrorPersistedKeypair,
};
use crate::key_manager::WorkerInfo;
use crate::KeyManagerError::RemoveErrorPersistedKeypair;
use fluence_keypair::{KeyFormat, KeyPair};
use fluence_libp2p::peerid_serializer;
use libp2p::PeerId;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::str::FromStr;

pub const fn default_bool<const V: bool>() -> bool {
    V
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PersistedKeypair {
    pub private_key_bytes: Vec<u8>,
    pub key_format: String,
}

#[derive(Serialize, Deserialize)]
pub struct PersistedWorker {
    #[serde(with = "peerid_serializer")]
    pub worker_id: PeerId,
    #[serde(with = "peerid_serializer")]
    pub deal_creator: PeerId,
    #[serde(default)]
    pub deal_id: String,
    #[serde(default = "default_bool::<true>")]
    pub active: bool,
}

impl From<PersistedWorker> for WorkerInfo {
    fn from(val: PersistedWorker) -> Self {
        WorkerInfo {
            deal_id: val.deal_id,
            creator: val.deal_creator,
            active: RwLock::new(val.active),
        }
    }
}

impl TryFrom<&KeyPair> for PersistedKeypair {
    type Error = KeyManagerError;

    fn try_from(keypair: &KeyPair) -> Result<Self, Self::Error> {
        Ok(Self {
            private_key_bytes: keypair.secret().map_err(|_| CannotExtractRSASecretKey)?,
            key_format: keypair.public().get_key_format().into(),
        })
    }
}

pub fn keypair_file_name(worker_id: PeerId) -> String {
    format!("{}_keypair.toml", worker_id.to_base58())
}

pub fn worker_file_name(worker_id: PeerId) -> String {
    format!("{}_info.toml", worker_id.to_base58())
}

fn is_keypair(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map_or(false, |n| n.ends_with("_keypair.toml"))
}

fn is_worker(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map_or(false, |n| n.ends_with("_info.toml"))
}

/// Persist keypair info to disk, so it is recreated after restart
pub fn persist_keypair(
    keypairs_dir: &Path,
    worker_id: PeerId,
    persisted_keypair: PersistedKeypair,
) -> Result<(), KeyManagerError> {
    let path = keypairs_dir.join(keypair_file_name(worker_id));
    let bytes =
        toml::to_vec(&persisted_keypair).map_err(|err| SerializePersistedKeypair { err })?;
    std::fs::write(&path, bytes).map_err(|err| WriteErrorPersistedKeypair { path, err })
}

/// Load info about persisted keypairs from disk
pub fn load_persisted_keypairs_and_workers(
    keypairs_dir: &Path,
) -> (Vec<KeyPair>, Vec<PersistedWorker>) {
    // Load all persisted service file names
    let files = match list_files(keypairs_dir) {
        Some(files) => files.collect(),
        None => {
            // Attempt to create directory
            if let Err(err) = create_dir(keypairs_dir) {
                log::warn!(
                    "{}",
                    CreateKeypairsDir {
                        path: keypairs_dir.to_path_buf(),
                        err,
                    }
                );
            }
            vec![]
        }
    };

    let mut keypairs = vec![];
    let mut workers = vec![];
    for file in files.iter() {
        let res: eyre::Result<()> = try {
            if is_keypair(file) {
                // Load persisted keypair
                let bytes = std::fs::read(file).map_err(|err| ReadPersistedKeypair {
                    err,
                    path: file.to_path_buf(),
                })?;
                let keypair: PersistedKeypair =
                    toml::from_slice(bytes.as_slice()).map_err(|err| {
                        DeserializePersistedKeypair {
                            err,
                            path: file.to_path_buf(),
                        }
                    })?;

                keypairs.push(KeyPair::from_secret_key(
                    keypair.private_key_bytes,
                    KeyFormat::from_str(&keypair.key_format)?,
                )?);
            } else if is_worker(file) {
                let bytes = std::fs::read(file).map_err(|err| ReadPersistedKeypair {
                    err,
                    path: file.to_path_buf(),
                })?;
                let worker: PersistedWorker =
                    toml::from_slice(bytes.as_slice()).map_err(|err| {
                        DeserializePersistedKeypair {
                            err,
                            path: file.to_path_buf(),
                        }
                    })?;
                workers.push(worker)
            }
        };

        if let Err(err) = res {
            log::warn!("{err}")
        }
    }

    (keypairs, workers)
}

pub fn remove_keypair(keypairs_dir: &Path, worker_id: PeerId) -> Result<(), KeyManagerError> {
    let path = keypairs_dir.join(keypair_file_name(worker_id));
    std::fs::remove_file(path.clone()).map_err(|err| RemoveErrorPersistedKeypair {
        path,
        worker_id,
        err,
    })
}

pub fn persist_worker(
    keypairs_dir: &Path,
    worker_id: PeerId,
    worker: PersistedWorker,
) -> Result<(), KeyManagerError> {
    let path = keypairs_dir.join(worker_file_name(worker_id));
    let bytes = toml::to_vec(&worker).map_err(|err| SerializePersistedKeypair { err })?;
    std::fs::write(&path, bytes).map_err(|err| WriteErrorPersistedKeypair { path, err })
}

pub fn remove_worker(keypairs_dir: &Path, worker_id: PeerId) -> Result<(), KeyManagerError> {
    let path = keypairs_dir.join(worker_file_name(worker_id));
    std::fs::remove_file(path.clone()).map_err(|err| RemoveErrorPersistedKeypair {
        path,
        worker_id,
        err,
    })
}
