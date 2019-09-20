/*
 * Copyright 2019 Fluence Labs Limited
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

use std::ops::AddAssign;

pub struct EnvModule {
    pub spent_gas: i64,
    pub eic: i64,
}

impl EnvModule {
    pub fn new() -> Box<Self> {
        Box::new(Self {
            spent_gas: 0i64,
            eic: 0i64,
        })
    }

    pub fn gas(&mut self, gas: i32) {
        // TODO: check for overflow
        self.spent_gas = self.spent_gas + gas as i64;
        //self.spent_gas.add_assign(gas as i64);
    }

    pub fn eic(&mut self, eic: i32) {
        // TODO: check for overflow
        self.eic.add_assign(eic as i64);
    }

    pub fn get_state(&self) -> (i64, i64) {
        (self.spent_gas, self.eic)
    }

    pub fn renew_state(&mut self) {
        self.spent_gas = 0;
        self.eic = 0;
    }
}

impl Default for EnvModule {
    fn default() -> Self {
        Self {
            spent_gas: 0i64,
            eic: 0i64,
        }
    }
}
