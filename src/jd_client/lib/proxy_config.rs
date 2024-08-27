use bitcoin::{Address, TxOut};
use key_utils::{Secp256k1PublicKey, Secp256k1SecretKey};
use roles_logic_sv2::{errors::Error, utils::CoinbaseOutput as CoinbaseOutput_};
use serde::Deserialize;
use std::{str::FromStr, time::Duration};

#[derive(Debug, Deserialize, Clone)]
pub struct CoinbaseOutput {
    output_script_type: String,
    output_script_value: String,
}

impl TryFrom<&CoinbaseOutput> for CoinbaseOutput_ {
    type Error = Error;

    fn try_from(pool_output: &CoinbaseOutput) -> Result<Self, Self::Error> {
        match pool_output.output_script_type.as_str() {
            "P2PK" | "P2PKH" | "P2WPKH" | "P2SH" | "P2WSH" | "P2TR" => Ok(CoinbaseOutput_ {
                output_script_type: pool_output.clone().output_script_type,
                output_script_value: pool_output.clone().output_script_value,
            }),
            _ => Err(Error::UnknownOutputScriptType),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct ProxyConfig {
    pub downstream_address: String,
    pub downstream_port: u16,
    pub max_supported_version: u16,
    pub min_supported_version: u16,
    pub min_extranonce2_size: u16,
    pub withhold: bool,
    pub authority_public_key: Secp256k1PublicKey,
    pub authority_secret_key: Secp256k1SecretKey,
    pub cert_validity_sec: u64,
    pub tp_address: String,
    pub tp_authority_public_key: Option<Secp256k1PublicKey>,
    pub retry: u32,
    pub upstreams: Vec<Upstream>,
    #[serde(deserialize_with = "duration_from_toml")]
    pub timeout: Duration,
    pub test_only_do_not_send_solution_to_tp: Option<bool>,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        let downstream_address =
            std::env::var("LISTEN_ON").unwrap_or_else(|_| "127.0.0.1".to_string());
        let address = std::env::var("ADDRESS").unwrap_or("".to_string());
        let pool_signature = "DEMANDsv2".to_string();
        let pool_signature = pool_signature + &address;
        Self {
            downstream_address,
            downstream_port: 34265,
            max_supported_version: 2,
            min_supported_version: 2,
            min_extranonce2_size: 8,
            withhold: false,
            authority_public_key: "9auqWEzQDVyd2oe1JVGFLMLHZtCo2FFqZwtKA5gd9xbuEu7PH72"
                .to_string()
                .try_into()
                .unwrap(),
            authority_secret_key: "mkDLTBBRxdBv998612qipDYoTK3YUrqLe8uWw7gu3iXbSrn2n"
                .to_string()
                .try_into()
                .unwrap(),
            cert_validity_sec: 3600,
            tp_address: "127.0.0.1:8442".to_string(),
            tp_authority_public_key: None,
            retry: 10,
            upstreams: vec![Upstream {
                authority_pubkey: "9bQHWXsQ2J9TRFTaxRh3KjoxdyLRfWVEy25YHtKF8y8gotLoCZZ"
                    .to_string()
                    .try_into()
                    .unwrap(),
                pool_address: "mining.dmnd.work:2000".to_string(),
                jd_address: "mining.dmnd.work:2000".to_string(),
                pool_signature,
            }],
            timeout: Duration::from_secs(1),
            test_only_do_not_send_solution_to_tp: None,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct Upstream {
    pub authority_pubkey: Secp256k1PublicKey,
    pub pool_address: String,
    pub jd_address: String,
    pub pool_signature: String, // string be included in coinbase tx input scriptsig
}

fn duration_from_toml<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct Helper {
        unit: String,
        value: u64,
    }

    let helper = Helper::deserialize(deserializer)?;
    match helper.unit.as_str() {
        "seconds" => Ok(Duration::from_secs(helper.value)),
        "secs" => Ok(Duration::from_secs(helper.value)),
        "s" => Ok(Duration::from_secs(helper.value)),
        "milliseconds" => Ok(Duration::from_millis(helper.value)),
        "millis" => Ok(Duration::from_millis(helper.value)),
        "ms" => Ok(Duration::from_millis(helper.value)),
        "microseconds" => Ok(Duration::from_micros(helper.value)),
        "micros" => Ok(Duration::from_micros(helper.value)),
        "us" => Ok(Duration::from_micros(helper.value)),
        "nanoseconds" => Ok(Duration::from_nanos(helper.value)),
        "nanos" => Ok(Duration::from_nanos(helper.value)),
        "ns" => Ok(Duration::from_nanos(helper.value)),
        // ... add other units as needed
        _ => Err(serde::de::Error::custom("Unsupported duration unit")),
    }
}

pub fn get_coinbase_output(address: &str) -> Result<Vec<TxOut>, Error> {
    let address =
        Address::from_str(address).unwrap_or_else(|_| panic!("Invalid address: {}", address));
    let script = address.script_pubkey();
    Ok(vec![TxOut {
        value: 0,
        script_pubkey: script,
    }])
}
