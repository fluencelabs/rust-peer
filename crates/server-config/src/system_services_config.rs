/*
 * Copyright 2023 Fluence Labs Limited
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

use super::defaults::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct SystemServicesConfig {
    #[serde(default)]
    pub disable: Vec<String>,
    #[serde(default)]
    pub aqua_ipfs: AquaIpfsConfig,
    #[serde(default)]
    pub decider: DeciderConfig,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AquaIpfsConfig {
    #[serde(default = "default_ipfs_multiaddr")]
    pub external_api_multiaddr: String,
    #[serde(default = "default_ipfs_multiaddr")]
    pub local_api_multiaddr: String,
}

impl Default for AquaIpfsConfig {
    fn default() -> Self {
        Self {
            external_api_multiaddr: default_ipfs_multiaddr(),
            local_api_multiaddr: default_ipfs_multiaddr(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct DeciderConfig {
    #[serde(default = "default_decider_spell_period_sec")]
    pub decider_period_sec: u32,
    #[serde(default = "default_worker_spell_period_sec")]
    pub worker_period_sec: u32,
    #[serde(default = "default_ipfs_multiaddr")]
    pub worker_ipfs_multiaddr: String,
    #[serde(default = "default_deal_network_api_endpoint")]
    pub network_api_endpoint: String,
    #[serde(default = "default_deal_contract_address_hex")]
    pub contract_address_hex: String,
    #[serde(default = "default_deal_contract_block_hex")]
    pub contract_block_hex: String,
}

impl Default for DeciderConfig {
    fn default() -> Self {
        Self {
            decider_period_sec: default_decider_spell_period_sec(),
            worker_period_sec: default_worker_spell_period_sec(),
            worker_ipfs_multiaddr: default_ipfs_multiaddr(),
            network_api_endpoint: default_deal_network_api_endpoint(),
            contract_address_hex: default_deal_contract_address_hex(),
            contract_block_hex: default_deal_contract_block_hex(),
        }
    }
}
