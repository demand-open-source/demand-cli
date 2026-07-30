use axum::http::StatusCode;
use bitcoin::{blockdata::transaction::Transaction, consensus::encode::deserialize_hex, Txid};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{error::Error as StdError, fmt, time::Duration};
use tokio::sync::Mutex;
use tracing::{debug, info};

const RPC_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const RPC_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) struct BitcoindRpc {
    url: String,
    user: String,
    pwd: String,
    fee_delta: i64,
    priority_transition_lock: Mutex<()>,
}

impl BitcoindRpc {
    pub(crate) fn new(url: String, user: String, pwd: String, fee_delta: i64) -> Self {
        Self {
            url,
            user,
            pwd,
            fee_delta,
            priority_transition_lock: Mutex::new(()),
        }
    }

    pub(crate) async fn update_transaction_priority(
        &self,
        tx: &str,
        prioritize: bool,
    ) -> Result<String, BitcoindRpcError> {
        // Serialize state reads and mutations so concurrent API calls cannot move a transaction
        // outside the two supported states.
        let _transition_guard = self.priority_transition_lock.lock().await;
        let tx = validate_transaction_hex(tx)?;
        let transaction = transaction_from_hex(tx)?;
        let txid = transaction.compute_txid();

        let current_delta = self.current_fee_delta(&txid).await?;
        let delta_to_apply = self.priority_delta(current_delta, prioritize)?;
        let next_delta = current_delta + delta_to_apply;

        if delta_to_apply != 0 {
            self.prioritise_transaction(&txid, delta_to_apply).await?;
        } else {
            info!(
                txid = %txid,
                prioritize,
                "transaction priority action is already at its boundary; no fee delta applied"
            );
        }

        // Bitcoin Core remembers a priority delta even when the transaction is not in its
        // mempool. Apply the delta first so a low-fee transaction is evaluated with its
        // modified fee when submitted. Deprioritizing never submits the transaction.
        if prioritize && !self.transaction_in_mempool(&txid.to_string()).await? {
            let (status, text) = self.send_request("sendrawtransaction", json!([tx])).await?;
            let submitted_txid =
                txid_from_sendrawtransaction_response(&transaction, status, &text)?;
            if submitted_txid != txid {
                return Err(BitcoindRpcError::InvalidResponse(format!(
                    "bitcoind returned txid {submitted_txid}, but transaction hex decodes to {txid}"
                )));
            }
        }

        if next_delta == self.fee_delta {
            crate::prioritized_transactions::record(transaction);
        } else {
            crate::prioritized_transactions::remove(&txid);
        }

        info!(
            txid = %txid,
            prioritize,
            current_delta,
            delta_to_apply,
            next_delta,
            "transaction priority state updated"
        );
        Ok(txid.to_string())
    }

    /// Calculates the adjustment needed to move between the only two supported states:
    ///
    /// | Current cumulative delta | Prioritize | Deprioritize |
    /// |--------------------------|------------|--------------|
    /// | `0`                      | `+fee_delta` | no-op       |
    /// | `fee_delta`              | no-op        | `-fee_delta` |
    ///
    /// The negative return value is only an adjustment that restores a prioritized
    /// transaction to zero. A negative cumulative fee delta is never a valid state.
    fn priority_delta(
        &self,
        current_delta: i64,
        prioritize: bool,
    ) -> Result<i64, BitcoindRpcError> {
        if ![0, self.fee_delta].contains(&current_delta) {
            return Err(BitcoindRpcError::Prioritize(format!(
                "transaction has unsupported fee delta {current_delta}; expected 0 or {}",
                self.fee_delta
            )));
        }

        Ok(match (current_delta, prioritize) {
            (0, true) => self.fee_delta,
            (current_delta, false) if current_delta == self.fee_delta => -self.fee_delta,
            _ => 0,
        })
    }

    async fn current_fee_delta(&self, txid: &Txid) -> Result<i64, BitcoindRpcError> {
        let (status, text) = self
            .send_request("getprioritisedtransactions", json!([]))
            .await?;
        let result = RpcResponse::from_response(status, &text)
            .map_err(|e| BitcoindRpcError::Prioritize(e.to_string()))?
            .ok_or_else(|| {
                BitcoindRpcError::InvalidResponse(format!(
                    "missing getprioritisedtransactions result from bitcoind: {text}"
                ))
            })?;
        let Some(priority) = result.get(txid.to_string()) else {
            return Ok(0);
        };

        priority
            .get("fee_delta")
            .and_then(Value::as_i64)
            .ok_or_else(|| {
                BitcoindRpcError::InvalidResponse(format!(
                    "missing or invalid fee_delta for transaction {txid}: {text}"
                ))
            })
    }

    async fn prioritise_transaction(
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
        }
    }

    pub(crate) async fn transaction_in_mempool(
        &self,
        txid: &str,
    ) -> Result<bool, BitcoindRpcError> {
        Ok(self.get_mempool_entry(txid).await?.is_some())
    }

    pub(crate) async fn mempool_entry_fees(
        &self,
        txid: &Txid,
    ) -> Result<Option<(f64, f64)>, BitcoindRpcError> {
        let Some(result) = self.get_mempool_entry(&txid.to_string()).await? else {
            return Ok(None);
        };

        let fees = result.get("fees").ok_or_else(|| {
            BitcoindRpcError::InvalidResponse(format!(
                "missing fees in getmempoolentry response from bitcoind: {result}"
            ))
        })?;
        let real_fee = fees.get("base").and_then(Value::as_f64).ok_or_else(|| {
            BitcoindRpcError::InvalidResponse(format!(
                "missing fees.base in getmempoolentry response from bitcoind: {result}"
            ))
        })?;
        let modified_fee = fees
            .get("modified")
            .and_then(Value::as_f64)
            .ok_or_else(|| {
                BitcoindRpcError::InvalidResponse(format!(
                    "missing fees.modified in getmempoolentry response from bitcoind: {result}"
                ))
            })?;

        Ok(Some((real_fee, modified_fee)))
    }

    async fn get_mempool_entry(&self, txid: &str) -> Result<Option<Value>, BitcoindRpcError> {
        let txid = validate_transaction_hex(txid)?;
        let (status, text) = self.send_request("getmempoolentry", json!([txid])).await?;
        let resp = RpcResponse::decode(status, &text)?;

        if let Some(error) = resp.error {
            if is_not_in_mempool_error(&error) {
                return Ok(None);
            }

            return Err(BitcoindRpcError::Other(format!(
                "bitcoind RPC error while checking mempool entry: {error}"
            )));
        }

        if !status.is_success() {
            return Err(BitcoindRpcError::Other(format!(
                "bitcoind HTTP {status}: {text}"
            )));
        }

        match resp.result {
            Some(result @ Value::Object(_)) => Ok(Some(result)),
            _ => Err(BitcoindRpcError::InvalidResponse(format!(
                "invalid getmempoolentry response from bitcoind: {text}"
            ))),
        }
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
            fee_delta = self.fee_delta,
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
                "transaction already in bitcoind mempool after its priority was applied"
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

fn validate_transaction_hex(tx: &str) -> Result<&str, BitcoindRpcError> {
    let tx = tx.trim();
    if tx.is_empty() {
        return Err(BitcoindRpcError::InvalidTransaction(
            "transaction hex cannot be empty".to_string(),
        ));
    }
    // Hex encoding represents each byte with two characters,
    // so valid transaction hex must have an even length.
    if tx.len() % 2 == 1 {
        return Err(BitcoindRpcError::InvalidTransaction(
            "Invalid transaction hash".to_string(),
        ));
    }
    if !tx.as_bytes().iter().all(|b| b.is_ascii_hexdigit()) {
        return Err(BitcoindRpcError::InvalidTransaction(
            "transaction must be hex encoded".to_string(),
        ));
    }
    Ok(tx)
}

fn is_already_in_mempool_error(error: &Value) -> bool {
    error
        .get("message")
        .and_then(Value::as_str)
        .is_some_and(|message| {
            let message = message.to_ascii_lowercase();
            message.contains("already in mempool") || message.contains("txn-already-in-mempool")
        })
}

fn is_not_in_mempool_error(error: &Value) -> bool {
    let code_is_not_found = error.get("code").and_then(Value::as_i64) == Some(-5);
    let message_says_not_in_mempool = error
        .get("message")
        .and_then(Value::as_str)
        .is_some_and(|message| message.to_ascii_lowercase().contains("not in mempool"));

    code_is_not_found || message_says_not_in_mempool
}

#[cfg(test)]
mod tests {
    use super::{
        is_already_in_mempool_error, is_not_in_mempool_error, transaction_from_hex, BitcoindRpc,
    };
    use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
    use serde_json::json;
    use serde_json::Value;
    use tokio::{
        sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
        task::JoinHandle,
    };

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

    #[derive(Clone)]
    struct MockPrioritizeState {
        requests: UnboundedSender<Value>,
        expected_txid: String,
        transaction_in_mempool: bool,
    }

    async fn mock_prioritize_bitcoind(
        State(state): State<MockPrioritizeState>,
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
            Some("getprioritisedtransactions") => (
                StatusCode::OK,
                Json(json!({
                    "result": {},
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
            Some("getmempoolentry") if state.transaction_in_mempool => (
                StatusCode::OK,
                Json(json!({
                    "result": {},
                    "error": null,
                    "id": "dmnd-client"
                })),
            ),
            Some("getmempoolentry") => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "result": null,
                    "error": {
                        "code": -5,
                        "message": "Transaction not in mempool"
                    },
                    "id": "dmnd-client"
                })),
            ),
            Some("sendrawtransaction") => (
                StatusCode::OK,
                Json(json!({
                    "result": state.expected_txid,
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

    async fn start_mock_prioritize_bitcoind(
        transaction_in_mempool: bool,
    ) -> (BitcoindRpc, UnboundedReceiver<Value>, JoinHandle<()>) {
        let expected_txid = transaction_from_hex(RAW_TX)
            .expect("valid test transaction")
            .compute_txid();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server should bind");
        let addr = listener.local_addr().expect("test server local addr");
        let (requests, received_requests) = unbounded_channel();
        let app = Router::new()
            .route("/", post(mock_prioritize_bitcoind))
            .with_state(MockPrioritizeState {
                requests,
                expected_txid: expected_txid.to_string(),
                transaction_in_mempool,
            });
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });
        let rpc = BitcoindRpc::new(
            format!("http://{addr}"),
            "user".to_string(),
            "password".to_string(),
            100_000_000,
        );

        (rpc, received_requests, server)
    }

    #[test]
    fn detects_bitcoind_not_in_mempool_error() {
        let error = json!({
            "code": -5,
            "message": "Transaction not in mempool"
        });

        assert!(is_not_in_mempool_error(&error));
    }

    #[test]
    fn ignores_unrelated_bitcoind_errors() {
        let error = json!({
            "code": -26,
            "message": "mandatory-script-verify-flag-failed"
        });

        assert!(!is_not_in_mempool_error(&error));
    }

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
    async fn prioritizing_does_not_submit_a_transaction_already_in_mempool() {
        let expected_txid = transaction_from_hex(RAW_TX)
            .expect("valid test transaction")
            .compute_txid();
        let (rpc, mut received_requests, server) = start_mock_prioritize_bitcoind(true).await;

        let txid = rpc
            .update_transaction_priority(RAW_TX, true)
            .await
            .expect("already-in-mempool tx should still be prioritized");

        assert_eq!(txid, expected_txid.to_string());

        let priorities_request = received_requests
            .recv()
            .await
            .expect("getprioritisedtransactions request");
        assert_eq!(priorities_request["method"], "getprioritisedtransactions");

        let prioritize_request = received_requests
            .recv()
            .await
            .expect("prioritisetransaction request");
        assert_eq!(prioritize_request["method"], "prioritisetransaction");
        assert_eq!(
            prioritize_request["params"],
            json!([expected_txid, 0, 100_000_000])
        );

        let mempool_request = received_requests
            .recv()
            .await
            .expect("getmempoolentry request");
        assert_eq!(mempool_request["method"], "getmempoolentry");
        assert_eq!(mempool_request["params"], json!([expected_txid]));
        assert!(
            received_requests.try_recv().is_err(),
            "an already-present transaction must not be submitted again"
        );

        server.abort();
    }

    #[tokio::test]
    async fn prioritizing_submits_a_missing_transaction_after_applying_the_delta() {
        let expected_txid = transaction_from_hex(RAW_TX)
            .expect("valid test transaction")
            .compute_txid();
        let (rpc, mut received_requests, server) = start_mock_prioritize_bitcoind(false).await;

        let txid = rpc
            .update_transaction_priority(RAW_TX, true)
            .await
            .expect("missing transaction should be prioritized before it is submitted");

        assert_eq!(txid, expected_txid.to_string());

        let expected_methods = [
            "getprioritisedtransactions",
            "prioritisetransaction",
            "getmempoolentry",
            "sendrawtransaction",
        ];
        for expected_method in expected_methods {
            let request = received_requests
                .recv()
                .await
                .expect("expected bitcoind request");
            assert_eq!(request["method"], expected_method);
        }
        assert!(
            received_requests.try_recv().is_err(),
            "prioritizing a missing transaction should issue exactly four requests"
        );

        server.abort();
    }

    #[tokio::test]
    async fn deprioritizing_does_not_submit_the_transaction() {
        #[derive(Clone)]
        struct MockBitcoindState {
            requests: UnboundedSender<Value>,
            prioritized_txid: String,
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
                Some("getprioritisedtransactions") => {
                    let mut priorities = serde_json::Map::new();
                    priorities.insert(state.prioritized_txid, json!({"fee_delta": 100_000_000}));
                    (
                        StatusCode::OK,
                        Json(json!({
                            "result": priorities,
                            "error": null,
                            "id": "dmnd-client"
                        })),
                    )
                }
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
                            "message": "unexpected method"
                        },
                        "id": "dmnd-client"
                    })),
                ),
            }
        }

        let expected_txid = transaction_from_hex(RAW_TX)
            .expect("valid test transaction")
            .compute_txid();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server should bind");
        let addr = listener.local_addr().expect("test server local addr");
        let (requests, mut received_requests) = unbounded_channel();
        let app = Router::new()
            .route("/", post(mock_bitcoind))
            .with_state(MockBitcoindState {
                requests,
                prioritized_txid: expected_txid.to_string(),
            });
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let rpc = BitcoindRpc::new(
            format!("http://{addr}"),
            "user".to_string(),
            "password".to_string(),
            100_000_000,
        );

        let txid = rpc
            .update_transaction_priority(RAW_TX, false)
            .await
            .expect("prioritized transaction should be restored to its original fee rate");

        assert_eq!(txid, expected_txid.to_string());

        let priorities_request = received_requests
            .recv()
            .await
            .expect("getprioritisedtransactions request");
        assert_eq!(priorities_request["method"], "getprioritisedtransactions");

        let deprioritize_request = received_requests
            .recv()
            .await
            .expect("prioritisetransaction request");
        assert_eq!(deprioritize_request["method"], "prioritisetransaction");
        assert_eq!(
            deprioritize_request["params"],
            json!([expected_txid, 0, -100_000_000])
        );
        assert!(
            received_requests.try_recv().is_err(),
            "deprioritizing must not send any additional request such as sendrawtransaction"
        );

        server.abort();
    }

    #[test]
    fn transaction_priority_actions_only_toggle_between_zero_and_configured_delta() {
        let rpc = BitcoindRpc::new(
            "http://127.0.0.1:8332".to_string(),
            "user".to_string(),
            "password".to_string(),
            100_000_000,
        );
        let cases = [
            (0, true, 100_000_000),
            (100_000_000, true, 0),
            (100_000_000, false, -100_000_000),
            (0, false, 0),
        ];

        for (current_delta, prioritize, expected_delta) in cases {
            assert_eq!(
                rpc.priority_delta(current_delta, prioritize).unwrap(),
                expected_delta
            );
        }
        assert!(rpc.priority_delta(-100_000_000, true).is_err());
        assert!(rpc.priority_delta(-100_000_000, false).is_err());
        assert!(rpc.priority_delta(1, true).is_err());
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
