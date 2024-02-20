use crate::function::{GetCommitmentFunction, GetStatusFunction, SubmitProofFunction};
use crate::{
    CurrentEpochFunction, DifficultyFunction, EpochDurationFunction, GetComputePeerFunction,
    GetComputeUnitsFunction, GetGlobalNonceFunction, InitTimestampFunction,
};
use chain_data::{next_opt, parse_chain_data, peer_id_to_bytes, FunctionTrait};
use chain_types::{
    Commitment, CommitmentId, CommitmentStatus, ComputePeer, ComputeUnit, GlobalNonce, Proof,
};
use clarity::Transaction;
use ethabi::ethereum_types::U256;
use ethabi::{ParamType, Token};
use eyre::eyre;
use fluence_libp2p::PeerId;
use futures::FutureExt;
use hex_utils::decode_hex;
use jsonrpsee::core::client::{BatchResponse, ClientT};
use jsonrpsee::core::params::{ArrayParams, BatchRequestBuilder};
use jsonrpsee::http_client::HttpClientBuilder;
use jsonrpsee::rpc_params;
use particle_args::{Args, JError};
use particle_builtins::{wrap, CustomService};
use particle_execution::{ParticleParams, ServiceFunction};
use serde_json::json;
use serde_json::Value as JValue;
use server_config::ChainConfig;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

const GAS_MULTIPLIER: f64 = 0.0;
pub struct ChainConnector {
    client: Arc<jsonrpsee::http_client::HttpClient>,
    config: ChainConfig,
    tx_nonce_mutex: Arc<Mutex<()>>,
    host_id: PeerId,
}

pub struct CCInitParams {
    pub difficulty: Vec<u8>,
    pub init_timestamp: U256,
    pub global_nonce: Vec<u8>,
    pub current_epoch: U256,
    pub epoch_duration: U256,
}

impl ChainConnector {
    pub fn new(
        config: ChainConfig,
        host_id: PeerId,
    ) -> eyre::Result<(Arc<Self>, HashMap<String, CustomService>)> {
        let connector = Arc::new(Self {
            client: Arc::new(HttpClientBuilder::default().build(&config.http_endpoint)?),
            config,
            tx_nonce_mutex: Arc::new(Default::default()),
            host_id,
        });

        let builtins = Self::make_connector_builtins(connector.clone());
        Ok((connector, builtins))
    }

    fn make_connector_builtins(connector: Arc<Self>) -> HashMap<String, CustomService> {
        let mut builtins = HashMap::new();
        builtins.insert(
            "connector".to_string(),
            CustomService::new(
                vec![("send_tx", Self::make_send_tx_closure(connector.clone()))],
                None,
            ),
        );
        builtins
    }

    fn make_send_tx_closure(connector: Arc<Self>) -> ServiceFunction {
        ServiceFunction::Immut(Box::new(move |args, params| {
            let connector = connector.clone();
            async move { wrap(connector.send_tx_builtin(args, params).await) }.boxed()
        }))
    }

    async fn send_tx_builtin(&self, args: Args, params: ParticleParams) -> Result<JValue, JError> {
        if params.init_peer_id != self.host_id {
            return Err(JError::new("Only the root worker can send transactions"));
        }

        let mut args = args.function_args.into_iter();
        let data: String = Args::next("data", &mut args)?;
        let to: String = Args::next("to", &mut args)?;
        let tx_hash = self
            .send_tx(decode_hex(&data)?, &to)
            .await
            .map_err(|err| JError::new(format!("Failed to send tx: {err}")))?;
        Ok(json!(tx_hash))
    }

    async fn get_gas_price(&self) -> eyre::Result<u128> {
        let resp: String = self.client.request("eth_gasPrice", rpc_params![]).await?;

        let mut tokens = parse_chain_data(&resp, &[ParamType::Uint(128)])?.into_iter();
        let price = next_opt(&mut tokens, "gas_price", Token::into_uint)?.as_u128();

        // increase price by GAS_MULTIPLIER so transaction are included faster
        let increase = (price as f64 * GAS_MULTIPLIER) as u128;
        let price = price.checked_add(increase).unwrap_or(price);

        Ok(price)
    }

    async fn get_tx_nonce(&self) -> eyre::Result<u128> {
        let address = self.config.wallet_key.to_address().to_string();
        let resp: String = self
            .client
            .request("eth_getTransactionCount", rpc_params![address, "pending"])
            .await?;

        let mut tokens = parse_chain_data(&resp, &[ParamType::Uint(128)])?.into_iter();
        let nonce = next_opt(&mut tokens, "nonce", Token::into_uint)?.as_u128();
        Ok(nonce)
    }

    async fn estimate_gas_limit(&self, data: &[u8], to: &str) -> eyre::Result<u128> {
        let response: String = self
            .client
            .request(
                "eth_estimateGas",
                rpc_params![json!({
                    "from": self.config.wallet_key.to_address().to_string(),
                    "to": to,
                    "data": format!("0x{}", hex::encode(data)),
                })],
            )
            .await?;
        let mut tokens = parse_chain_data(&response, &[ParamType::Uint(128)])?.into_iter();
        Ok(next_opt(&mut tokens, "gas_limit", Token::into_uint)?.as_u128())
    }

    pub async fn send_tx(&self, data: Vec<u8>, to: &str) -> eyre::Result<String> {
        // We use this lock no ensure that we don't send two transactions with the same nonce
        let _lock = self.tx_nonce_mutex.lock().await;
        let nonce = self.get_tx_nonce().await?;
        let gas_price = self.get_gas_price().await?;
        let gas_limit = self.estimate_gas_limit(&data, to).await?;
        // Create a new transaction
        let tx = Transaction::Legacy {
            nonce: nonce.into(),
            gas_price: gas_price.into(),
            gas_limit: gas_limit.into(),
            to: to.parse()?,
            value: 0u32.into(),
            data,
            signature: None, // Not signed. Yet.
        };

        let tx = tx
            .sign(&self.config.wallet_key, Some(self.config.network_id))
            .to_bytes();
        let tx = hex::encode(tx);

        let resp: String = self
            .client
            .request("eth_sendRawTransaction", rpc_params![format!("0x{}", tx)])
            .await?;
        Ok(resp)
    }

    pub async fn get_current_commitment_id(&self) -> eyre::Result<Option<CommitmentId>> {
        let peer_id = Token::FixedBytes(peer_id_to_bytes(self.host_id));
        let data = GetComputePeerFunction::data(&[peer_id])?;
        let resp: String = self
            .client
            .request(
                "eth_call",
                rpc_params![json!({
                    "data": data,
                    "to": self.config.market_contract_address,
                })],
            )
            .await?;
        Ok(ComputePeer::from(&resp)?.commitment_id)
    }

    pub async fn get_commitment_status(
        &self,
        commitment_id: CommitmentId,
    ) -> eyre::Result<CommitmentStatus> {
        let data = GetStatusFunction::data(&[Token::FixedBytes(commitment_id.0)])?;
        let resp: String = self
            .client
            .request(
                "eth_call",
                rpc_params![json!({
                    "data": data,
                    "to": self.config.cc_contract_address,
                })],
            )
            .await?;
        CommitmentStatus::from(&resp)
    }

    pub async fn get_commitment(&self, commitment_id: CommitmentId) -> eyre::Result<Commitment> {
        let data = GetCommitmentFunction::data(&[Token::FixedBytes(commitment_id.0)])?;
        let resp: String = self
            .client
            .request(
                "eth_call",
                rpc_params![json!({
                    "data": data,
                    "to": self.config.cc_contract_address,
                })],
            )
            .await?;
        Commitment::from(&resp)
    }

    pub async fn get_global_nonce(&self) -> eyre::Result<GlobalNonce> {
        let data = GetGlobalNonceFunction::data(&[])?;
        let resp: String = self
            .client
            .request(
                "eth_call",
                rpc_params![json!({
                    "data": data,
                    "to": self.config.cc_contract_address
                })],
            )
            .await?;

        Ok(GlobalNonce(GetGlobalNonceFunction::decode_bytes(&resp)?))
    }

    pub async fn submit_proof(&self, proof: Proof) -> eyre::Result<String> {
        let data = SubmitProofFunction::data_bytes(&[
            Token::FixedBytes(proof.unit_id.0),
            Token::FixedBytes(proof.local_unit_nonce),
            Token::FixedBytes(proof.target_hash),
        ])?;

        self.send_tx(data, &self.config.cc_contract_address).await
    }

    pub async fn get_compute_units(&self) -> eyre::Result<Vec<ComputeUnit>> {
        let data =
            GetComputeUnitsFunction::data(&[Token::FixedBytes(peer_id_to_bytes(self.host_id))])?;
        let resp: String = self
            .client
            .request(
                "eth_call",
                rpc_params![json!({
                    "data": data,
                    "to": self.config.market_contract_address,
                })],
            )
            .await?;
        let mut tokens =
            parse_chain_data(&resp, &GetComputeUnitsFunction::signature())?.into_iter();
        let units = next_opt(&mut tokens, "units", Token::into_array)?.into_iter();
        let compute_units = units
            .map(ComputeUnit::from_token)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(compute_units)
    }

    pub async fn get_cc_init_params(&self) -> eyre::Result<CCInitParams> {
        let mut batch = BatchRequestBuilder::new();

        batch.insert("eth_call", self.difficulty_params()?)?;
        batch.insert("eth_call", self.init_timestamp_params()?)?;
        batch.insert("eth_call", self.global_nonce_params()?)?;
        batch.insert("eth_call", self.current_epoch_params()?)?;
        batch.insert("eth_call", self.epoch_duration_params()?)?;

        let resp: BatchResponse<String> = self.client.batch_request(batch).await?;
        let mut results = resp
            .into_ok()
            .map_err(|err| eyre!("Some request failed in a batch {err:?}"))?;

        let difficulty = DifficultyFunction::decode_bytes(
            &results.next().ok_or(eyre!("No response for difficulty"))?,
        )?;
        let init_timestamp = InitTimestampFunction::decode_uint(
            &results
                .next()
                .ok_or(eyre!("No response for init_timestamp"))?,
        )?;
        let global_nonce = GetGlobalNonceFunction::decode_bytes(
            &results
                .next()
                .ok_or(eyre!("No response for global_nonce"))?,
        )?;
        let current_epoch = CurrentEpochFunction::decode_uint(
            &results
                .next()
                .ok_or(eyre!("No response for current_epoch"))?,
        )?;
        let epoch_duration = EpochDurationFunction::decode_uint(
            &results
                .next()
                .ok_or(eyre!("No response for epoch_duration"))?,
        )?;

        Ok(CCInitParams {
            difficulty,
            init_timestamp,
            global_nonce,
            current_epoch,
            epoch_duration,
        })
    }

    fn difficulty_params(&self) -> eyre::Result<ArrayParams> {
        let data = DifficultyFunction::data(&[])?;
        Ok(rpc_params![
            json!({"data": data, "to": self.config.cc_contract_address})
        ])
    }

    fn init_timestamp_params(&self) -> eyre::Result<ArrayParams> {
        let data = InitTimestampFunction::data(&[])?;
        Ok(rpc_params![
            json!({"data": data, "to": self.config.core_contract_address})
        ])
    }
    fn global_nonce_params(&self) -> eyre::Result<ArrayParams> {
        let data = GetGlobalNonceFunction::data(&[])?;
        Ok(rpc_params![
            json!({"data": data, "to": self.config.cc_contract_address})
        ])
    }
    fn current_epoch_params(&self) -> eyre::Result<ArrayParams> {
        let data = CurrentEpochFunction::data(&[])?;
        Ok(rpc_params![
            json!({"data": data, "to": self.config.core_contract_address})
        ])
    }
    fn epoch_duration_params(&self) -> eyre::Result<ArrayParams> {
        let data = EpochDurationFunction::data(&[])?;
        Ok(rpc_params![
            json!({"data": data, "to": self.config.core_contract_address})
        ])
    }
}

#[cfg(test)]
mod tests {
    use crate::ChainConnector;
    use chain_data::peer_id_from_hex;
    use chain_types::CommitmentId;
    use clarity::PrivateKey;
    use std::str::FromStr;
    use std::sync::Arc;

    fn get_connector(url: &str) -> Arc<ChainConnector> {
        let (connector, _) = ChainConnector::new(
            server_config::ChainConfig {
                http_endpoint: url.to_string(),
                ws_endpoint: "".to_string(),
                cc_contract_address: "0x3Aa5ebB10DC797CAC828524e59A333d0A371443c".to_string(),
                core_contract_address: "0x0B306BF915C4d645ff596e518fAf3F9669b97016".to_string(),
                market_contract_address: "0x68B1D87F95878fE05B998F19b66F4baba5De1aed".to_string(),

                network_id: 0,
                wallet_key: PrivateKey::from_str(
                    "0xfdc4ba94809c7930fe4676b7d845cbf8fa5c1beae8744d959530e5073004cf3f",
                )
                .unwrap(),
                ccp_endpoint: "".to_string(),
            },
            peer_id_from_hex("0x6497db93b32e4cdd979ada46a23249f444da1efb186cd74b9666bd03f710028b")
                .unwrap(),
        )
        .unwrap();

        connector
    }
    #[tokio::test]
    async fn test_get_compute_units() {
        let expected_data = "0x000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000025d204dcc21f59c2a2098a277e48879207f614583e066654ad6736d36815ebb9e00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000450e2f2a5bdb528895e9005f67e70fe213b9b822122e96fd85d2238cae55b6f900000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
        let expected_response =
            format!("{{\"jsonrpc\":\"2.0\",\"result\":\"{expected_data}\",\"id\":0}}");

        let mut server = mockito::Server::new();
        let url = server.url();
        let mock = server
            .mock("POST", "/")
            // expect exactly 1 POST request
            .expect(1)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(expected_response)
            .create();

        let units = get_connector(&url).get_compute_units().await.unwrap();

        mock.assert();
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].start_epoch, 0.into());
        assert!(units[0].deal.is_none());
        assert_eq!(units[1].start_epoch, 0.into());
        assert!(units[1].deal.is_none());
    }

    #[tokio::test]
    async fn test_get_current_commitment_id_none() {
        let expected_data = "0xaa3046a12a1aac6e840625e6329d70b427328fec36dc8d273e5e6454b85633d5000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000020000000000000000000000005b73c5498c1e3b4dba84de0f1833c4a029d90519";
        let expected_response =
            format!("{{\"jsonrpc\":\"2.0\",\"result\":\"{expected_data}\",\"id\":0}}");

        let mut server = mockito::Server::new();
        let url = server.url();
        let mock = server
            .mock("POST", "/")
            // expect exactly 1 POST request
            .expect(1)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(expected_response)
            .create();
        let commitment_id = get_connector(&url)
            .get_current_commitment_id()
            .await
            .unwrap();

        mock.assert();
        assert!(commitment_id.is_none());
    }

    #[tokio::test]
    async fn test_get_current_commitment_id_some() {
        let expected_data = "0xaa3046a12a1aac6e840625e6329d70b427328fec36dc8d273e5e6454b85633d5aa3046a12a1aac6e840625e6329d70b427328feceedc8d273e5e6454b85633b5000000000000000000000000000000000000000000000000000000000000000a0000000000000000000000005b73c5498c1e3b4dba84de0f1833c4a029d90519";
        let expected_response =
            format!("{{\"jsonrpc\":\"2.0\",\"result\":\"{expected_data}\",\"id\":0}}");

        let mut server = mockito::Server::new();
        let url = server.url();
        let mock = server
            .mock("POST", "/")
            // expect exactly 1 POST request
            .expect(1)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(expected_response)
            .create();
        let commitment_id = get_connector(&url)
            .get_current_commitment_id()
            .await
            .unwrap();

        mock.assert();
        assert!(commitment_id.is_some());
        assert_eq!(
            hex::encode(commitment_id.unwrap().0),
            "aa3046a12a1aac6e840625e6329d70b427328feceedc8d273e5e6454b85633b5"
        );
    }

    #[tokio::test]
    async fn test_get_commitment() {
        let commitment_id = "0xa98dc43600773b162bcdb8175eadc037412cd7ad83555fafa507702011a53992";

        let expected_data = "0x00000000000000000000000000000000000000000000000000000000000000016497db93b32e4cdd979ada46a23249f444da1efb186cd74b9666bd03f710028b000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000012c00000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
        let expected_response =
            format!("{{\"jsonrpc\":\"2.0\",\"result\":\"{expected_data}\",\"id\":0}}");
        let mut server = mockito::Server::new();
        let url = server.url();
        let mock = server
            .mock("POST", "/")
            // expect exactly 1 POST request
            .expect(1)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(expected_response)
            .create();
        let commitment_id = CommitmentId(hex::decode(&commitment_id[2..]).unwrap());
        let commitment = get_connector(&url)
            .get_commitment(commitment_id)
            .await
            .unwrap();

        mock.assert();
        assert_eq!(
            commitment.status,
            chain_types::CommitmentStatus::WaitDelegation
        );
        assert_eq!(commitment.start_epoch, 0.into());
        assert_eq!(commitment.end_epoch, 300.into());
    }

    #[tokio::test]
    async fn get_commitment_status() {
        let commitment_id = "0xa98dc43600773b162bcdb8175eadc037412cd7ad83555fafa507702011a53992";

        let expected_data = "0x0000000000000000000000000000000000000000000000000000000000000001";
        let expected_response =
            format!("{{\"jsonrpc\":\"2.0\",\"result\":\"{expected_data}\",\"id\":0}}");
        let mut server = mockito::Server::new();
        let url = server.url();
        let mock = server
            .mock("POST", "/")
            // expect exactly 1 POST request
            .expect(1)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(expected_response)
            .create();
        let commitment_id = CommitmentId(hex::decode(&commitment_id[2..]).unwrap());
        let status = get_connector(&url)
            .get_commitment_status(commitment_id)
            .await
            .unwrap();

        mock.assert();
        assert_eq!(status, chain_types::CommitmentStatus::WaitDelegation);
    }

    #[tokio::test]
    async fn test_batch_init_request() {
        let expected_response = r#"[
          {
            "jsonrpc": "2.0",
            "result": "0x76889c92f61b9c5df216e048df56eb8f4eb02f172ab0d5b04edb9190ab9c9eec",
            "id": 0
          },
          {
            "jsonrpc": "2.0",
            "result": "0x0000000000000000000000000000000000000000000000000000000065ca5a01",
            "id": 1
          },
          {
            "jsonrpc": "2.0",
            "result": "0x0000000000000000000000000000000000000000000000000000000000000005",
            "id": 2
          },
          {
            "jsonrpc": "2.0",
            "result": "0x00000000000000000000000000000000000000000000000000000000000016be",
            "id": 3
          },
          {
            "jsonrpc": "2.0",
            "result": "0x000000000000000000000000000000000000000000000000000000000000000f",
            "id": 4
          }
        ]"#;
        let mut server = mockito::Server::new();
        let url = server.url();
        let mock = server
            .mock("POST", "/")
            // expect exactly 1 POST request
            .expect(1)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(expected_response)
            .create();

        let init_params = get_connector(&url).get_cc_init_params().await.unwrap();

        mock.assert();
        assert_eq!(
            hex::encode(&init_params.difficulty),
            "76889c92f61b9c5df216e048df56eb8f4eb02f172ab0d5b04edb9190ab9c9eec"
        );
        assert_eq!(init_params.init_timestamp, 1707760129.into());
        assert_eq!(
            hex::encode(init_params.global_nonce),
            "0000000000000000000000000000000000000000000000000000000000000005"
        );
        assert_eq!(
            init_params.current_epoch,
            0x00000000000000000000000000000000000000000000000000000000000016be.into()
        );
        assert_eq!(
            init_params.epoch_duration,
            0x000000000000000000000000000000000000000000000000000000000000000f.into()
        );
    }
}
