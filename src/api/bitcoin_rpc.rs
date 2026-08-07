use axum::http::StatusCode;
use bitcoin::{
    blockdata::transaction::Transaction,
    consensus::encode::{deserialize_hex, serialize_hex},
    Txid,
};
use bitcoincore_rpc::{Auth, Client};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    error::Error as StdError,
    fmt,
    sync::Arc,
    time::Duration,
};
use tracing::{debug, info};

const RPC_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const RPC_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct PrioTx {
    #[serde(skip)]
    pub(crate) txid: Txid,
    pub(crate) fee_delta: i64,
    pub(crate) in_mempool: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) modified_fee: Option<i64>,
}

pub(crate) struct BitcoindRpc {
    url: String,
    user: String,
    pwd: String,
}

impl BitcoindRpc {
    pub(crate) fn new(url: String, user: String, pwd: String) -> Self {
        Self { url, user, pwd }
    }

    pub(crate) async fn submit_transaction(
        &self,
        transaction: &Transaction,
    ) -> Result<String, BitcoindRpcError> {
        let transaction_hex = serialize_hex(transaction);
        let (status, text) = self
            .send_request("sendrawtransaction", json!([transaction_hex]))
            .await?;
        let txid = txid_from_sendrawtransaction_response(transaction, status, &text)?;
        let computed_txid = transaction.compute_txid();
        if txid != computed_txid {
            return Err(BitcoindRpcError::InvalidResponse(format!(
                "bitcoind returned txid {txid}, but transaction hex decodes to {computed_txid}"
            )));
        }

        Ok(txid.to_string())
    }

    pub(crate) async fn get_prioritised_transactions(
        &self,
    ) -> Result<HashMap<Txid, PrioTx>, BitcoindRpcError> {
        let (status, text) = self
            .send_request("getprioritisedtransactions", json!([]))
            .await?;

        let transactions = match RpcResponse::from_response(status, &text)? {
            Some(Value::Object(transactions)) => transactions,
            _ => {
                return Err(BitcoindRpcError::InvalidResponse(format!(
                    "invalid getprioritisedtransactions response from bitcoind: {text}"
                )))
            }
        };

        transactions
            .into_iter()
            .map(|(txid, priority)| {
                let parsed_txid = txid.parse::<Txid>().map_err(|error| {
                    BitcoindRpcError::InvalidResponse(format!(
                        "invalid txid in getprioritisedtransactions response from bitcoind: {txid}: {error}"
                    ))
                })?;
                let fee_delta = priority
                    .get("fee_delta")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| {
                        BitcoindRpcError::InvalidResponse(format!(
                            "invalid fee_delta for {txid} in getprioritisedtransactions response from bitcoind"
                        ))
                    })?;
                let in_mempool = priority
                    .get("in_mempool")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| {
                        BitcoindRpcError::InvalidResponse(format!(
                            "invalid in_mempool for {txid} in getprioritisedtransactions response from bitcoind"
                        ))
                    })?;
                let modified_fee = priority
                    .get("modified_fee")
                    .map(|modified_fee| {
                        modified_fee.as_i64().ok_or_else(|| {
                            BitcoindRpcError::InvalidResponse(format!(
                                "invalid modified_fee for {txid} in getprioritisedtransactions response from bitcoind"
                            ))
                        })
                    })
                    .transpose()?;

                Ok((
                    parsed_txid,
                    PrioTx {
                        txid: parsed_txid,
                        fee_delta,
                        in_mempool,
                        modified_fee,
                    },
                ))
            })
            .collect()
    }

    pub(crate) async fn prioritise_transaction(
        &self,
        txid: &Txid,
        fee_delta: i64,
    ) -> Result<(), BitcoindRpcError> {
        let (status, text) = self
            .send_request(
                "prioritisetransaction",
                json!([txid.to_string(), 0, fee_delta]),
            )
            .await?;
        info!(
            txid = %txid,
            fee_delta,
            %status,
            response = %text,
            "bitcoind prioritisetransaction response"
        );

        let result = RpcResponse::from_response(status, &text)
            .map_err(|e| BitcoindRpcError::Prioritize(e.to_string()))?;

        match result.and_then(|value| value.as_bool()) {
            Some(true) => Ok(()),
            Some(false) => Err(BitcoindRpcError::Prioritize(format!(
                "bitcoind returned false for prioritisetransaction: {text}"
            ))),
            None => Err(BitcoindRpcError::Prioritize(format!(
                "invalid prioritisetransaction response from bitcoind: {text}"
            ))),
        }?;
        Ok(())
    }

    async fn send_request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<(StatusCode, String), BitcoindRpcError> {
        let body = json!({
            "jsonrpc": "1.0",
            "id": "dmnd-client",
            "method": method,
            "params": params
        });

        debug!(
            method,
            url = self.url.as_str(),
            rpc_user = self.user.as_str(),
            "sending bitcoind RPC request"
        );

        let client = reqwest::Client::builder()
            .connect_timeout(RPC_CONNECT_TIMEOUT)
            .timeout(RPC_REQUEST_TIMEOUT)
            .build()
            .expect("Failed to build client");

        let response = client
            .post(&self.url)
            .basic_auth(&self.user, Some(&self.pwd))
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let text = response.text().await?;
        debug!(method, %status, response = %text, "received bitcoind RPC response");
        Ok((status, text))
    }
}

#[allow(dead_code)]
fn txid_from_sendrawtransaction_response(
    transaction: &Transaction,
    status: StatusCode,
    text: &str,
) -> Result<Txid, BitcoindRpcError> {
    let resp = RpcResponse::decode(status, text)?;

    if let Some(error) = resp.error.as_ref() {
        if is_already_in_mempool_error(error) {
            let txid = transaction.compute_txid();
            info!(
                txid = %txid,
                response = %text,
                "transaction already in bitcoind mempool; prioritizing existing transaction"
            );
            return Ok(txid);
        }

        return Err(BitcoindRpcError::Rejected(format!(
            "bitcoind RPC error: {error}"
        )));
    }

    if !status.is_success() {
        return Err(BitcoindRpcError::Other(format!(
            "bitcoind HTTP {status}: {text}"
        )));
    }

    resp.result
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or_else(|| {
            BitcoindRpcError::InvalidResponse("empty response from bitcoind".to_string())
        })?
        .parse::<Txid>()
        .map_err(|e| BitcoindRpcError::InvalidResponse(format!("invalid txid from bitcoind: {e}")))
}

#[allow(dead_code)]
fn transaction_from_hex(tx: &str) -> Result<Transaction, BitcoindRpcError> {
    deserialize_hex(tx).map_err(|e| {
        BitcoindRpcError::InvalidTransaction(format!("failed to decode transaction hex: {e}"))
    })
}

#[derive(Deserialize, Debug)]
struct RpcResponse {
    result: Option<Value>,
    error: Option<Value>,
}

impl RpcResponse {
    fn from_response(status: StatusCode, text: &str) -> Result<Option<Value>, BitcoindRpcError> {
        let resp = Self::decode(status, text)?;
        if !status.is_success() {
            return Err(BitcoindRpcError::Other(format!(
                "bitcoind HTTP {status}: {text}"
            )));
        }
        if let Some(err) = resp.error {
            return Err(BitcoindRpcError::Rejected(format!(
                "bitcoind RPC error: {err}"
            )));
        }
        Ok(resp.result)
    }

    fn decode(status: StatusCode, text: &str) -> Result<Self, BitcoindRpcError> {
        match serde_json::from_str(text) {
            Ok(r) => Ok(r),
            Err(e) if status.is_success() => Err(BitcoindRpcError::InvalidResponse(format!(
                "failed to decode bitcoind response ({e}): {text}"
            ))),
            Err(_) => Err(BitcoindRpcError::Other(format!(
                "bitcoind HTTP {status}: {text}"
            ))),
        }
    }
}

#[allow(dead_code)]
fn is_already_in_mempool_error(error: &Value) -> bool {
    error
        .get("message")
        .and_then(Value::as_str)
        .is_some_and(|message| {
            let message = message.to_ascii_lowercase();
            message.contains("already in mempool") || message.contains("txn-already-in-mempool")
        })
}

#[cfg(test)]
mod tests {
    use super::{is_already_in_mempool_error, transaction_from_hex, BitcoindRpc};
    use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
    use serde_json::json;
    use serde_json::Value;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

    const RAW_TX: &str = concat!(
        "01000000",
        "01",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "ffffffff",
        "00",
        "ffffffff",
        "01",
        "0000000000000000",
        "00",
        "00000000",
    );

    #[test]
    fn detects_bitcoind_already_in_mempool_error() {
        let error = json!({
            "code": -27,
            "message": "txn-already-in-mempool"
        });

        assert!(is_already_in_mempool_error(&error));
    }

    #[test]
    fn ignores_already_in_chain_error_for_mempool_detection() {
        let error = json!({
            "code": -27,
            "message": "Transaction already in block chain"
        });

        assert!(!is_already_in_mempool_error(&error));
    }

    #[tokio::test]
    async fn priority_rpc_methods_use_expected_requests() {
        #[derive(Clone)]
        struct MockBitcoindState {
            requests: UnboundedSender<Value>,
        }

        async fn mock_bitcoind(
            State(state): State<MockBitcoindState>,
            Json(body): Json<Value>,
        ) -> (StatusCode, Json<Value>) {
            let method = body
                .get("method")
                .and_then(Value::as_str)
                .map(str::to_owned);
            state
                .requests
                .send(body)
                .expect("test should receive bitcoind request");

            match method.as_deref() {
                Some("sendrawtransaction") => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "result": null,
                        "error": {
                            "code": -27,
                            "message": "txn-already-in-mempool"
                        },
                        "id": "dmnd-client"
                    })),
                ),
                Some("getprioritisedtransactions") => (
                    StatusCode::OK,
                    Json(json!({
                        "result": {
                            "2fb7d2ab4ea206f3491ae234583c124d5087b6267308288e1359a6052fc477e1": {
                                "fee_delta": 100_000_000,
                                "in_mempool": true,
                                "modified_fee": 100_010_000
                            }
                        },
                        "error": null,
                        "id": "dmnd-client"
                    })),
                ),
                Some("prioritisetransaction") => (
                    StatusCode::OK,
                    Json(json!({
                        "result": true,
                        "error": null,
                        "id": "dmnd-client"
                    })),
                ),
                _ => (
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "result": null,
                        "error": {
                            "code": -32601,
                            "message": "unknown method"
                        },
                        "id": "dmnd-client"
                    })),
                ),
            }
        }

        let transaction = transaction_from_hex(RAW_TX).expect("valid test transaction");
        let expected_txid = transaction.compute_txid();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server should bind");
        let addr = listener.local_addr().expect("test server local addr");
        let (requests, mut received_requests) = unbounded_channel();
        let app = Router::new()
            .route("/", post(mock_bitcoind))
            .with_state(MockBitcoindState { requests });
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let rpc = BitcoindRpc::new(
            format!("http://{addr}"),
            "user".to_string(),
            "password".to_string(),
        );

        let prioritised_transactions = rpc
            .get_prioritised_transactions()
            .await
            .expect("getprioritisedtransactions should return the priority map");
        assert_eq!(
            prioritised_transactions,
            std::collections::HashMap::from([(
                expected_txid,
                super::PrioTx {
                    txid: expected_txid,
                    fee_delta: 100_000_000,
                    in_mempool: true,
                    modified_fee: Some(100_010_000),
                },
            )])
        );
        let get_prioritised_request = received_requests
            .recv()
            .await
            .expect("getprioritisedtransactions request");
        assert_eq!(
            get_prioritised_request["method"],
            "getprioritisedtransactions"
        );
        assert_eq!(get_prioritised_request["params"], json!([]));

        let txid = rpc
            .submit_transaction(&transaction)
            .await
            .expect("already-in-mempool response should be accepted");

        assert_eq!(txid, expected_txid.to_string());

        let submit_request = received_requests
            .recv()
            .await
            .expect("sendrawtransaction request");
        assert_eq!(submit_request["method"], "sendrawtransaction");
        assert_eq!(submit_request["params"], json!([RAW_TX]));

        rpc.prioritise_transaction(&expected_txid, 100_000_000)
            .await
            .expect("fee delta adjustment should be applied");

        let prioritize_request = received_requests
            .recv()
            .await
            .expect("prioritisetransaction request");
        assert_eq!(prioritize_request["method"], "prioritisetransaction");
        assert_eq!(
            prioritize_request["params"],
            json!([expected_txid, 0, 100_000_000])
        );
        assert!(received_requests.try_recv().is_err());

        server.abort();
    }
}

#[derive(Clone, Debug)]
pub(crate) enum BitcoindRpcError {
    InvalidTransaction(String),
    Rejected(String),
    Timeout(String),
    Other(String),
    InvalidResponse(String),
    Prioritize(String),
}

impl BitcoindRpcError {
    pub(crate) fn status_code(&self) -> StatusCode {
        match self {
            BitcoindRpcError::InvalidTransaction(_) | BitcoindRpcError::Rejected(_) => {
                StatusCode::BAD_REQUEST
            }
            BitcoindRpcError::Timeout(_) => StatusCode::GATEWAY_TIMEOUT,
            BitcoindRpcError::Other(_)
            | BitcoindRpcError::InvalidResponse(_)
            | BitcoindRpcError::Prioritize(_) => StatusCode::BAD_GATEWAY,
        }
    }
}

impl From<reqwest::Error> for BitcoindRpcError {
    fn from(error: reqwest::Error) -> Self {
        let source = error
            .source()
            .map(|source| format!("; source: {source}"))
            .unwrap_or_default();
        if error.is_timeout() {
            BitcoindRpcError::Timeout(format!(
                "timed out while connecting to bitcoind: {error}{source}"
            ))
        } else if error.is_builder() {
            BitcoindRpcError::Other(format!(
                "failed to build bitcoind RPC request: {error}{source}"
            ))
        } else {
            BitcoindRpcError::Other(format!("failed to connect to bitcoind: {error}{source}"))
        }
    }
}

impl fmt::Display for BitcoindRpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BitcoindRpcError::InvalidTransaction(msg)
            | BitcoindRpcError::Rejected(msg)
            | BitcoindRpcError::Timeout(msg)
            | BitcoindRpcError::Other(msg)
            | BitcoindRpcError::InvalidResponse(msg)
            | BitcoindRpcError::Prioritize(msg) => f.write_str(msg),
        }
    }
}
