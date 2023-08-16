use fluence_libp2p::PeerId;
use fluence_spell_dtos::trigger_config::{TriggerConfig, TriggerConfigValue};
use fluence_spell_dtos::value::{ScriptValue, SpellValueT, U32Value, UnitValue};
use particle_execution::FunctionOutcome;
use particle_services::ParticleAppServices;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("Spell {spell_id} not found (function {function_name})")]
    ServiceNotFound {
        spell_id: String,
        function_name: String,
    },
    #[error("Call {spell_id}.{function_name} didn't return any result")]
    EmptyResult {
        spell_id: String,
        function_name: String,
    },
    #[error("Error while calling {spell_id}.{function_name}: {reason}")]
    OtherError {
        spell_id: String,
        function_name: String,
        reason: String,
    },
    #[error("Result of the call {spell_id}.{function_name} cannot be parsed to the {target_type} type: {reason}")]
    ResultParseError {
        spell_id: String,
        function_name: String,
        target_type: &'static str,
        reason: String,
    },
    #[error("Call {spell_id}.{function_name} executed with the error: {reason}")]
    ExecutionError {
        spell_id: String,
        function_name: String,
        reason: String,
    },
}

struct Function {
    name: &'static str,
    args: Vec<Value>,
}

// how to name this pair
#[derive(Clone)]
struct SpellAddr {
    worker_id: PeerId,
    spell_id: String,
}

struct SpellServiceApi {
    services: ParticleAppServices,
}

impl SpellServiceApi {
    pub fn new(services: ParticleAppServices) -> Self {
        Self { services }
    }

    pub fn set_script(&self, addr: SpellAddr, script: String, ttl: u64) -> Result<(), CallError> {
        let function = Function {
            name: "set_script_source_to_file",
            args: vec![json!(script)],
        };
        let _ = self.call::<UnitValue>(addr, function, ttl)?;
        Ok(())
    }
    pub fn get_script(&self, addr: SpellAddr, ttl: u64) -> Result<String, CallError> {
        let function = Function {
            name: "get_script_source_from_file",
            args: vec![],
        };
        let script_value = self.call::<ScriptValue>(addr, function, ttl)?;
        Ok(script_value.source_code)
    }

    pub fn set_trigger_config(
        &self,
        addr: SpellAddr,
        config: TriggerConfig,
        ttl: u64,
    ) -> Result<(), CallError> {
        let function = Function {
            name: "set_trigger_config",
            args: vec![json!(config)],
        };
        let _ = self.call::<UnitValue>(addr, function, ttl)?;
        Ok(())
    }

    pub fn get_trigger_config(
        &self,
        addr: SpellAddr,
        ttl: u64,
    ) -> Result<TriggerConfig, CallError> {
        let function = Function {
            name: "get_trigger_config",
            args: vec![],
        };
        let trigger_config_value = self.call::<TriggerConfigValue>(addr, function, ttl)?;
        Ok(trigger_config_value.config)
    }

    // TODO: use `Map<String, Value>` for init_data instead of `Value`
    pub fn update_kv(&self, addr: SpellAddr, kv_data: Value, ttl: u64) -> Result<(), CallError> {
        let function = Function {
            name: "set_json_fields",
            args: vec![json!(kv_data.to_string())],
        };
        let _ = self.call::<UnitValue>(addr, function, ttl)?;
        Ok(())
    }

    /// Load the counter (how many times the spell was run)
    pub fn get_counter(&self, addr: SpellAddr, ttl: u64) -> Result<u32, CallError> {
        let function = Function {
            name: "get_u32",
            args: vec![json!("counter")],
        };
        let result = self.call::<U32Value>(addr, function, ttl)?;
        Ok(result.num)
    }
    /// Update the counter (how many times the spell was run)
    /// TODO: permission check here or not?
    pub fn set_counter(&self, addr: SpellAddr, counter: u64, ttl: u64) -> Result<(), CallError> {
        let function = Function {
            name: "set_u32",
            args: vec![json!("counter"), json!(counter)],
        };
        let _ = self.call::<UnitValue>(addr, function, ttl)?;

        Ok(())
    }

    pub fn set_trigger_event(
        &self,
        addr: SpellAddr,
        event: Value,
        ttl: u64,
    ) -> Result<(), CallError> {
        let kv_data = json! ({
            "trigger": event
        });
        self.update_kv(addr, kv_data, ttl)
    }

    // pub fn push_mailbox_message(&self, addr: SpellAddr, ttl: u64) {}

    fn call<T>(&self, addr: SpellAddr, function: Function, ttl: u64) -> Result<T, CallError>
    where
        T: DeserializeOwned + SpellValueT,
    {
        use CallError::*;
        let spell_id = addr.spell_id;
        let result = self.services.call_function(
            addr.worker_id,
            &spell_id,
            function.name,
            function.args,
            None,
            addr.worker_id,
            Duration::from_millis(ttl),
        );
        match result {
            FunctionOutcome::NotDefined { .. } => Err(ServiceNotFound {
                spell_id,
                function_name: function.name.to_string(),
            }),
            FunctionOutcome::Empty => Err(EmptyResult {
                spell_id,
                function_name: function.name.to_string(),
            }),
            FunctionOutcome::Err(err) => Err(OtherError {
                spell_id,
                function_name: function.name.to_string(),
                reason: err.to_string(),
            }),
            FunctionOutcome::Ok(value) => match serde_json::from_value::<T>(value) {
                Ok(result) if result.is_success() => Ok(result),
                Ok(result) => Err(ExecutionError {
                    spell_id,
                    function_name: function.name.to_string(),
                    reason: result.take_error(),
                }),
                Err(e) => Err(ResultParseError {
                    spell_id,
                    function_name: function.name.to_string(),
                    target_type: std::any::type_name::<T>(),
                    reason: e.to_string(),
                }),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    use particle_services::{ParticleAppServices, ServiceType};

    use fluence_libp2p::PeerId;
    use libp2p_identity::Keypair;
    use tempdir::TempDir;

    use config_utils::to_peer_id;
    use particle_modules::ModuleRepository;
    use server_config::ServicesConfig;

    use fluence_spell_dtos::trigger_config::TriggerConfig;
    use fluence_spell_dtos::value::*;
    use maplit::hashmap;
    use serde_json::json;

    use crate::{SpellAddr, SpellServiceApi};

    const TTL: u64 = 100000;

    fn create_pid() -> PeerId {
        let keypair = Keypair::generate_ed25519();

        PeerId::from(keypair.public())
    }

    fn create_pas(
        local_pid: PeerId,
        management_pid: PeerId,
        base_dir: PathBuf,
    ) -> (ParticleAppServices, ModuleRepository) {
        let startup_kp = Keypair::generate_ed25519();
        let vault_dir = base_dir.join("..").join("vault");
        let max_heap_size = server_config::default_module_max_heap_size();
        let config = ServicesConfig::new(
            local_pid,
            base_dir,
            vault_dir,
            HashMap::new(),
            management_pid,
            to_peer_id(&startup_kp),
            max_heap_size,
            None,
            Default::default(),
        )
        .unwrap();

        let repo = ModuleRepository::new(
            &config.modules_dir,
            &config.blueprint_dir,
            &config.particles_vault_dir,
            max_heap_size,
            None,
            Default::default(),
        );

        let pas = ParticleAppServices::new(config, repo.clone(), None, None);
        (pas, repo)
    }

    fn create_spell(
        pas: &ParticleAppServices,
        blueprint_id: String,
        worker_id: PeerId,
    ) -> Result<String, String> {
        pas.create_service(ServiceType::Spell, blueprint_id, worker_id, worker_id)
            .map_err(|e| e.to_string())
    }

    fn setup() -> (SpellServiceApi, SpellAddr) {
        let base_dir = TempDir::new("test3").unwrap();
        let local_pid = create_pid();
        let management_pid = create_pid();
        let (pas, repo) = create_pas(local_pid, management_pid, base_dir.into_path());

        let api = SpellServiceApi::new(pas.clone());
        let (storage, _) = spell_storage::SpellStorage::create(Path::new(""), &pas, &repo).unwrap();
        let spell_service_blueprint_id = storage.get_blueprint();
        let spell_id = create_spell(&pas, spell_service_blueprint_id, local_pid).unwrap();
        let addr = crate::SpellAddr {
            worker_id: local_pid,
            spell_id,
        };
        (api, addr)
    }

    #[test]
    fn test_counter() {
        let (api, addr) = setup();
        let result1 = api.get_counter(addr.clone(), TTL);
        assert!(
            result1.is_ok(),
            "must be able to get a counter of an empty spell"
        );
        assert_eq!(
            result1.unwrap(),
            0,
            "the counter of an empty spell must be zero"
        );
        let new_counter = 7;
        let result2 = api.set_counter(addr.clone(), new_counter, TTL);
        assert!(
            result2.is_ok(),
            "must be able to set a counter of an empty spell"
        );
        let result3 = api.get_counter(addr, TTL);
        assert!(
            result3.is_ok(),
            "must be able to get a counter of an empty spell again"
        );
        assert_eq!(
            result3.unwrap(),
            new_counter as u32,
            "must be able to load an updated counter"
        );
    }

    #[test]
    fn test_script() {
        let (api, addr) = setup();
        let script_original = "(noop)".to_string();
        let result1 = api.set_script(addr.clone(), script_original.clone(), TTL);
        assert!(result1.is_ok(), "must be able to update script");
        let script = api.get_script(addr, TTL);
        assert!(script.is_ok(), "must be able to load script");
        assert_eq!(script.unwrap(), script_original, "scripts must be equal");
    }

    #[test]
    fn test_trigger_config() {
        let (api, addr) = setup();
        let trigger_config_original = TriggerConfig::default();
        let result1 = api.set_trigger_config(addr.clone(), trigger_config_original.clone(), TTL);
        assert!(result1.is_ok(), "must be able to set trigger config");
        let result2 = api.get_trigger_config(addr, TTL);
        assert!(result2.is_ok(), "must be able to get trigger config");
        assert_eq!(
            result2.unwrap(),
            trigger_config_original,
            "trigger configs must be equal"
        );
    }

    #[test]
    fn test_kv() {
        let (api, addr) = setup();
        let init_data = hashmap! {
            "a1" => json!(1),
            "b1" => json!("test"),
        };
        let result1 = api.update_kv(addr.clone(), json!(init_data), TTL);
        assert!(result1.is_ok(), "must be able to update kv");

        let function = super::Function {
            name: "get_string",
            args: vec![json!("a1")],
        };
        let result = api.call::<StringValue>(addr.clone(), function, TTL);
        assert!(result.is_ok(), "must be able to add get_string");
        assert_eq!(result.unwrap().str, "1", "must be able to add get_string");

        let function = super::Function {
            name: "get_string",
            args: vec![json!("b1")],
        };
        let result = api.call::<StringValue>(addr, function, TTL);
        assert!(result.is_ok(), "must be able to add get_string");
        assert_eq!(
            result.unwrap().str,
            "\"test\"",
            "must be able to add get_string"
        );
    }

    #[test]
    fn test_trigger_event() {
        let (api, addr) = setup();
        let trigger_event = json!({
            "peer": json!([]),
            "timer": vec![json!({
                "timestamp": 1
            })]
        });
        let result = api.set_trigger_event(addr.clone(), trigger_event.clone(), TTL);
        assert!(result.is_ok(), "must be able to set trigger event");

        let function = super::Function {
            name: "get_string",
            args: vec![json!("trigger")],
        };
        let result = api.call::<StringValue>(addr, function, TTL);
        assert!(result.is_ok(), "must be able to add get_string");
        let trigger_event_read: Result<serde_json::Value, _> =
            serde_json::from_str(&result.unwrap().str);
        assert!(
            trigger_event_read.is_ok(),
            "read trigger event must be parsable"
        );
        assert_eq!(
            trigger_event_read.unwrap(),
            trigger_event,
            "read trigger event must be equal to the original one"
        );
    }
}
