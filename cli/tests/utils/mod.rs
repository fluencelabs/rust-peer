use fluence::credentials::Credentials;
use fluence::register::Register;
use fluence::publisher::Publisher;

use std::error::Error;

use rand::Rng;
use ethkey::Secret;
use web3::types::{Address, H256};

type Result<T> = std::result::Result<T, Box<Error>>;

#[derive(Debug)]
pub struct TestOpts {
    pub contract_address: Address,
    pub account: Address,
    pub start_port: u16,
    pub credentials: Credentials,
    pub eth_url: String,
    pub last_used_port: Option<u16>,
    pub gas: u32,
    pub code_bytes: Vec<u8>,
    pub swarm_url: String,
}

impl TestOpts {
    pub fn default() -> TestOpts {
        TestOpts {
            contract_address: "9995882876ae612bfd829498ccd73dd962ec950a".parse().unwrap(),
            account: "4180fc65d613ba7e1a385181a219f1dbfe7bf11d".parse().unwrap(),
            start_port: 25000,
            credentials: Credentials::No,
            eth_url: String::from("http://localhost:8545/"),
            last_used_port: None,
            gas: 1_000_000,
            code_bytes: vec![1, 2, 3],
            swarm_url: String::from("http://localhost:8500"),
        }
    }

    pub fn new(
        contract_address: Address,
        account: Address,
        start_port: u16,
        credentials: Credentials,
        eth_url: String,
        gas: u32,
        code_bytes: Vec<u8>,
        swarm_url: String,
    ) -> TestOpts {
        TestOpts {
            contract_address,
            account,
            start_port,
            credentials,
            eth_url,
            last_used_port: None,
            gas,
            code_bytes,
            swarm_url,
        }
    }

    pub fn register_node(&mut self, ports: u8, private: bool) -> Result<Register> {
        let mut rng = rand::thread_rng();
        let rnd_num: u64 = rng.gen();
        let tendermint_key: H256 = H256::from(rnd_num);

        let start_port = self.last_used_port.unwrap_or(self.start_port);
        let end_port = start_port + ports;

        self.last_used_port = Some(end_port + 1);

        let reg = Register::new(
            "127.0.0.1".parse().unwrap(),
            tendermint_key,
            start_port,
            end_port,
            self.contract_address,
            self.account,
            self.eth_url,
            self.credentials,
            false,
            self.gas,
            private,
        ).unwrap();

        reg.register(false);

        Ok(reg)
    }

    pub fn publish_app(self, cluster_size: u8, pin_to: Vec<H256>) -> Result<Publisher> {
        let publish = Publisher::new(
            self.code_bytes,
            self.contract_address,
            self.account,
            self.swarm_url,
            self.eth_url,
            self.credentials,
            cluster_size,
            self.gas,
            pin_to
        );

        publish.publish(false);

        Ok(publish)
    }
}
