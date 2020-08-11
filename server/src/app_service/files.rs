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

use crate::app_service::Blueprint;

/// Calculates filename of the config for a wasm module
pub(super) fn module_config_name<S: AsRef<str>>(module: S) -> String {
    format!("{}_config.toml", module.as_ref())
}

/// Calculates filename of wasm module
pub(super) fn module_file_name<S: AsRef<str>>(module: S) -> String {
    format!("{}.wasm", module.as_ref())
}

/// Calculates filename of the blueprint
pub(super) fn blueprint_file_name(blueprint: &Blueprint) -> String {
    format!("{}_blueprint.toml", blueprint.id)
}

/// Returns true if file is named like a blueprint would be
pub(super) fn is_blueprint(name: &String) -> bool {
    name.ends_with("_blueprint.toml")
}

/// Returns true if file is named like a module would be
pub(super) fn is_module(name: &String) -> bool {
    name.ends_with(".wasm")
}
