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

#![warn(rust_2018_idioms)]
#![deny(
    dead_code,
    nonstandard_style,
    unused_imports,
    unused_mut,
    unused_variables,
    unused_unsafe,
    unreachable_patterns
)]

pub use wasmer_wit::types::InterfaceType as IType;
pub use wasmer_wit::values::InterfaceValue as IValue;
pub use wasmer_wit::vec1;

use serde_json::{json, Value};
use vec1::Vec1;

pub fn into_str(v: &IValue) -> Option<&str> {
    if let IValue::String(s) = v {
        Some(s.as_str())
    } else {
        None
    }
}

pub fn into_string(v: IValue) -> Option<String> {
    if let IValue::String(s) = v {
        Some(s)
    } else {
        None
    }
}

pub fn as_record_opt(v: std::result::Result<Option<Value>, Value>) -> Option<IValue> {
    match v {
        Ok(None) => unit(),
        Ok(Some(v)) => ok(v),
        Err(e) => error(e),
    }
}

pub fn as_record(v: std::result::Result<Value, Value>) -> Option<IValue> {
    match v {
        Ok(v) => ok(v),
        Err(e) => error(e),
    }
}

pub fn ok(value: Value) -> Option<IValue> {
    let value = IValue::String(value.to_string());
    Some(IValue::Record(
        Vec1::new(vec![IValue::U32(0), value]).unwrap(),
    ))
}

pub fn ivalue_ok(value: IValue) -> Option<IValue> {
    ok(ivalue_to_jvalue(value))
}

pub fn error(err: Value) -> Option<IValue> {
    let err = IValue::String(err.to_string());
    Some(IValue::Record(
        Vec1::new(vec![IValue::U32(1), err]).unwrap(),
    ))
}

pub fn unit() -> Option<IValue> {
    Some(IValue::Record(
        Vec1::new(vec![IValue::S32(0), IValue::String("\"\"".to_string())]).unwrap(),
    ))
}

/// Serializes IValue to json bytes
fn ivalue_to_jvalue(v: IValue) -> Value {
    match v {
        IValue::S8(v) => json!(v),
        IValue::S16(v) => json!(v),
        IValue::S32(v) => json!(v),
        IValue::S64(v) => json!(v),
        IValue::U8(v) => json!(v),
        IValue::U16(v) => json!(v),
        IValue::U32(v) => json!(v),
        IValue::U64(v) => json!(v),
        IValue::F32(v) => json!(v),
        IValue::F64(v) => json!(v),
        IValue::String(v) => json!(v),
        IValue::I32(v) => json!(v),
        IValue::I64(v) => json!(v),
        IValue::Array(v) => json!(v.into_iter().map(ivalue_to_jvalue).collect::<Vec<_>>()),
        IValue::Record(v) => json!(v
            .into_vec()
            .into_iter()
            .map(ivalue_to_jvalue)
            .collect::<Vec<_>>()),
    }
}
