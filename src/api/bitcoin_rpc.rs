use axum::http::StatusCode;
use bitcoin::{blockdata::transaction::Transaction, consensus::encode::deserialize_hex, Txid};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{error::Error as StdError, fmt, time::Duration};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

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
    ) -> Result<String, PriorityUpdateError> {
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
            self.apply_priority_delta(
                &transaction,
                current_delta,
                delta_to_apply,
                if prioritize {
                    PriorityUpdateOperation::Prioritize
                } else {
                    PriorityUpdateOperation::Deprioritize
                },
            )
            .await?;
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
        if prioritize {
            let transaction_in_mempool = match self.transaction_in_mempool(&txid.to_string()).await
            {
                Ok(in_mempool) => in_mempool,
                Err(source) => {
                    return Err(self
                        .fail_after_priority_applied(
                            &transaction,
                            current_delta,
                            delta_to_apply,
                            PriorityUpdateOperation::CheckMempool,
                            source,
                        )
                        .await);
                }
            };

            if !transaction_in_mempool {
                let submission_result = async {
                    let (status, text) =
                        self.send_request("sendrawtransaction", json!([tx])).await?;
                    let submitted_txid =
                        txid_from_sendrawtransaction_response(&transaction, status, &text)?;
                    if submitted_txid != txid {
                        return Err(BitcoindRpcError::InvalidResponse(format!(
                            "bitcoind returned txid {submitted_txid}, but transaction hex decodes to {txid}"
                        )));
                    }
                    Ok(())
                }
                .await;

                if let Err(source) = submission_result {
                    if source.may_have_applied_mutation() {
                        match self.transaction_in_mempool(&txid.to_string()).await {
                            Ok(true) => {
                                warn!(
                                    txid = %txid,
                                    error = %source,
                                    "sendrawtransaction returned an ambiguous error, but reconciliation found the transaction in the mempool"
                                );
                            }
                            Ok(false) => {
                                return Err(self
                                    .fail_after_priority_applied(
                                        &transaction,
                                        current_delta,
                                        delta_to_apply,
                                        PriorityUpdateOperation::Submit,
                                        source,
                                    )
                                    .await);
                            }
                            Err(reconciliation_error) => {
                                return Err(self
                                    .indeterminate_submission_error(
                                        &transaction,
                                        current_delta,
                                        delta_to_apply,
                                        source,
                                        reconciliation_error,
                                    )
                                    .await);
                            }
                        }
                    } else {
                        return Err(self
                            .fail_after_priority_applied(
                                &transaction,
                                current_delta,
                                delta_to_apply,
                                PriorityUpdateOperation::Submit,
                                source,
                            )
                            .await);
                    }
                }
            }
        }

        self.sync_transaction_tracking(&transaction, next_delta);

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

    async fn apply_priority_delta(
        &self,
        transaction: &Transaction,
        current_delta: i64,
        delta_to_apply: i64,
        operation: PriorityUpdateOperation,
    ) -> Result<(), PriorityUpdateError> {
        let txid = transaction.compute_txid();
        let target_delta = current_delta + delta_to_apply;
        let Err(source) = self.prioritise_transaction(&txid, delta_to_apply).await else {
            return Ok(());
        };

        if !source.may_have_applied_mutation() {
            return Err(PriorityUpdateError::MutationFailed {
                operation,
                source,
                observed_delta: Some(current_delta),
            });
        }

        match self.current_fee_delta(&txid).await {
            Ok(observed_delta) if observed_delta == target_delta => {
                warn!(
                    txid = %txid,
                    %operation,
                    error = %source,
                    observed_delta,
                    "priority mutation returned an ambiguous error, but reconciliation confirmed it succeeded"
                );
                self.sync_transaction_tracking(transaction, observed_delta);
                Ok(())
            }
            Ok(observed_delta) if observed_delta == current_delta => {
                self.sync_transaction_tracking(transaction, observed_delta);
                Err(PriorityUpdateError::MutationFailed {
                    operation,
                    source,
                    observed_delta: Some(observed_delta),
                })
            }
            Ok(observed_delta) => {
                self.sync_transaction_tracking(transaction, observed_delta);
                Err(PriorityUpdateError::StateIndeterminate {
                    operation,
                    source,
                    details: format!(
                        "reconciliation observed unsupported fee delta {observed_delta}; expected {current_delta} or {target_delta}"
                    ),
                })
            }
            Err(reconciliation_error) => Err(PriorityUpdateError::StateIndeterminate {
                operation,
                source,
                details: format!(
                    "could not reconcile the fee delta after the ambiguous error: {reconciliation_error}"
                ),
            }),
        }
    }

    async fn fail_after_priority_applied(
        &self,
        transaction: &Transaction,
        original_delta: i64,
        applied_delta: i64,
        operation: PriorityUpdateOperation,
        source: BitcoindRpcError,
    ) -> PriorityUpdateError {
        if applied_delta == 0 {
            return PriorityUpdateError::OperationFailed { operation, source };
        }

        match self
            .rollback_priority_delta(transaction, original_delta, applied_delta)
            .await
        {
            RollbackOutcome::Restored => PriorityUpdateError::OperationRolledBack {
                operation,
                source,
            },
            RollbackOutcome::Failed {
                rollback_error,
                observed_delta,
            } => PriorityUpdateError::RollbackFailed {
                operation,
                source,
                rollback_error,
                observed_delta,
            },
            RollbackOutcome::Indeterminate {
                rollback_error,
                reconciliation_error,
            } => PriorityUpdateError::StateIndeterminate {
                operation,
                source,
                details: format!(
                    "rollback also failed ambiguously ({rollback_error}), and its result could not be reconciled: {reconciliation_error}"
                ),
            },
        }
    }

    async fn indeterminate_submission_error(
        &self,
        transaction: &Transaction,
        original_delta: i64,
        applied_delta: i64,
        source: BitcoindRpcError,
        reconciliation_error: BitcoindRpcError,
    ) -> PriorityUpdateError {
        let rollback_details = if applied_delta == 0 {
            "no new priority delta was applied, so no rollback was required".to_string()
        } else {
            match self
                .rollback_priority_delta(transaction, original_delta, applied_delta)
                .await
            {
                RollbackOutcome::Restored => {
                    "the newly applied priority delta was rolled back".to_string()
                }
                RollbackOutcome::Failed {
                    rollback_error,
                    observed_delta,
                } => format!(
                    "priority rollback failed ({rollback_error}); reconciliation observed fee delta {observed_delta}"
                ),
                RollbackOutcome::Indeterminate {
                    rollback_error,
                    reconciliation_error,
                } => format!(
                    "priority rollback failed ambiguously ({rollback_error}) and could not be reconciled ({reconciliation_error})"
                ),
            }
        };

        PriorityUpdateError::StateIndeterminate {
            operation: PriorityUpdateOperation::Submit,
            source,
            details: format!(
                "could not determine whether the transaction reached the mempool ({reconciliation_error}); {rollback_details}"
            ),
        }
    }

    async fn rollback_priority_delta(
        &self,
        transaction: &Transaction,
        original_delta: i64,
        applied_delta: i64,
    ) -> RollbackOutcome {
        let txid = transaction.compute_txid();
        match self.prioritise_transaction(&txid, -applied_delta).await {
            Ok(()) => {
                self.sync_transaction_tracking(transaction, original_delta);
                RollbackOutcome::Restored
            }
            Err(rollback_error) => {
                error!(
                    txid = %txid,
                    error = %rollback_error,
                    "failed to roll back newly applied transaction priority delta"
                );
                match self.current_fee_delta(&txid).await {
                    Ok(observed_delta) if observed_delta == original_delta => {
                        warn!(
                            txid = %txid,
                            error = %rollback_error,
                            observed_delta,
                            "priority rollback returned an error, but reconciliation confirmed the original delta was restored"
                        );
                        self.sync_transaction_tracking(transaction, observed_delta);
                        RollbackOutcome::Restored
                    }
                    Ok(observed_delta) => {
                        self.sync_transaction_tracking(transaction, observed_delta);
                        RollbackOutcome::Failed {
                            rollback_error,
                            observed_delta,
                        }
                    }
                    Err(reconciliation_error) => RollbackOutcome::Indeterminate {
                        rollback_error,
                        reconciliation_error,
                    },
                }
            }
        }
    }

    fn sync_transaction_tracking(&self, transaction: &Transaction, observed_delta: i64) {
        let txid = transaction.compute_txid();
        if observed_delta == self.fee_delta {
            crate::prioritized_transactions::record(transaction.clone());
        } else if observed_delta == 0 {
            crate::prioritized_transactions::remove(&txid);
        }
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

        let response = RpcResponse::decode(status, &text)?;
        if let Some(error) = response.error {
            return Err(BitcoindRpcError::Prioritize(format!(
                "bitcoind RPC error while updating transaction priority: {error}"
            )));
        }
        if !status.is_success() {
            return Err(BitcoindRpcError::Other(format!(
                "bitcoind HTTP {status}: {text}"
            )));
        }

        match response.result.and_then(|value| value.as_bool()) {
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
    use std::sync::{
        atomic::{AtomicBool, AtomicI64, Ordering},
        Arc,
    };
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

    #[derive(Clone, Copy)]
    enum SubmissionBehavior {
        Rejected,
        AmbiguousApplied,
        AmbiguousNotApplied,
    }

    #[derive(Clone, Copy)]
    struct FailurePathConfig {
        transaction_in_mempool: bool,
        mempool_check_fails: bool,
        submission_behavior: SubmissionBehavior,
        ambiguous_prioritize_response: bool,
        rollback_fails: bool,
    }

    #[derive(Clone)]
    struct FailurePathState {
        requests: UnboundedSender<Value>,
        expected_txid: String,
        fee_delta: Arc<AtomicI64>,
        transaction_in_mempool: Arc<AtomicBool>,
        config: FailurePathConfig,
    }

    fn mock_rpc_response(result: Value, error: Value) -> String {
        json!({
            "result": result,
            "error": error,
            "id": "dmnd-client"
        })
        .to_string()
    }

    async fn mock_failure_path_bitcoind(
        State(state): State<FailurePathState>,
        Json(body): Json<Value>,
    ) -> (StatusCode, String) {
        let method = body
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);
        state
            .requests
            .send(body.clone())
            .expect("test should receive bitcoind request");

        match method.as_deref() {
            Some("getprioritisedtransactions") => {
                let fee_delta = state.fee_delta.load(Ordering::SeqCst);
                let mut priorities = serde_json::Map::new();
                if fee_delta != 0 {
                    priorities.insert(
                        state.expected_txid.clone(),
                        json!({ "fee_delta": fee_delta }),
                    );
                }
                (
                    StatusCode::OK,
                    mock_rpc_response(Value::Object(priorities), Value::Null),
                )
            }
            Some("prioritisetransaction") => {
                let delta = body
                    .get("params")
                    .and_then(Value::as_array)
                    .and_then(|params| params.get(2))
                    .and_then(Value::as_i64)
                    .expect("prioritisetransaction fee delta");

                if delta < 0 && state.config.rollback_fails {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        mock_rpc_response(
                            Value::Null,
                            json!({
                                "code": -1,
                                "message": "rollback rejected"
                            }),
                        ),
                    );
                }

                state.fee_delta.fetch_add(delta, Ordering::SeqCst);
                if delta > 0 && state.config.ambiguous_prioritize_response {
                    (StatusCode::OK, "{".to_string())
                } else {
                    (
                        StatusCode::OK,
                        mock_rpc_response(Value::Bool(true), Value::Null),
                    )
                }
            }
            Some("getmempoolentry") if state.config.mempool_check_fails => (
                StatusCode::INTERNAL_SERVER_ERROR,
                mock_rpc_response(
                    Value::Null,
                    json!({
                        "code": -1,
                        "message": "mempool query failed"
                    }),
                ),
            ),
            Some("getmempoolentry") if state.transaction_in_mempool.load(Ordering::SeqCst) => {
                (StatusCode::OK, mock_rpc_response(json!({}), Value::Null))
            }
            Some("getmempoolentry") => (
                StatusCode::INTERNAL_SERVER_ERROR,
                mock_rpc_response(
                    Value::Null,
                    json!({
                        "code": -5,
                        "message": "Transaction not in mempool"
                    }),
                ),
            ),
            Some("sendrawtransaction") => match state.config.submission_behavior {
                SubmissionBehavior::Rejected => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    mock_rpc_response(
                        Value::Null,
                        json!({
                            "code": -26,
                            "message": "mandatory-script-verify-flag-failed"
                        }),
                    ),
                ),
                SubmissionBehavior::AmbiguousApplied => {
                    state.transaction_in_mempool.store(true, Ordering::SeqCst);
                    (StatusCode::OK, "{".to_string())
                }
                SubmissionBehavior::AmbiguousNotApplied => (StatusCode::OK, "{".to_string()),
            },
            _ => (
                StatusCode::BAD_REQUEST,
                mock_rpc_response(
                    Value::Null,
                    json!({
                        "code": -32601,
                        "message": "unknown method"
                    }),
                ),
            ),
        }
    }

    async fn start_failure_path_bitcoind(
        config: FailurePathConfig,
    ) -> (
        BitcoindRpc,
        UnboundedReceiver<Value>,
        FailurePathState,
        JoinHandle<()>,
    ) {
        let expected_txid = transaction_from_hex(RAW_TX)
            .expect("valid test transaction")
            .compute_txid();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server should bind");
        let addr = listener.local_addr().expect("test server local addr");
        let (requests, received_requests) = unbounded_channel();
        let state = FailurePathState {
            requests,
            expected_txid: expected_txid.to_string(),
            fee_delta: Arc::new(AtomicI64::new(0)),
            transaction_in_mempool: Arc::new(AtomicBool::new(config.transaction_in_mempool)),
            config,
        };
        let app = Router::new()
            .route("/", post(mock_failure_path_bitcoind))
            .with_state(state.clone());
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

        (rpc, received_requests, state, server)
    }

    async fn assert_request_methods(
        received_requests: &mut UnboundedReceiver<Value>,
        expected_methods: &[&str],
    ) {
        for expected_method in expected_methods {
            let request = received_requests
                .recv()
                .await
                .expect("expected bitcoind request");
            assert_eq!(request["method"], *expected_method);
        }
        assert!(
            received_requests.try_recv().is_err(),
            "mock received an unexpected extra RPC request"
        );
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
    async fn definitive_submission_rejection_rolls_back_the_new_delta() {
        let (rpc, mut received_requests, state, server) =
            start_failure_path_bitcoind(FailurePathConfig {
                transaction_in_mempool: false,
                mempool_check_fails: false,
                submission_behavior: SubmissionBehavior::Rejected,
                ambiguous_prioritize_response: false,
                rollback_fails: false,
            })
            .await;

        let error = rpc
            .update_transaction_priority(RAW_TX, true)
            .await
            .expect_err("rejected transaction submission should fail");

        assert!(matches!(
            error,
            super::PriorityUpdateError::OperationRolledBack {
                operation: super::PriorityUpdateOperation::Submit,
                ..
            }
        ));
        assert_eq!(state.fee_delta.load(Ordering::SeqCst), 0);
        assert_request_methods(
            &mut received_requests,
            &[
                "getprioritisedtransactions",
                "prioritisetransaction",
                "getmempoolentry",
                "sendrawtransaction",
                "prioritisetransaction",
            ],
        )
        .await;

        server.abort();
    }

    #[tokio::test]
    async fn mempool_check_failure_rolls_back_the_new_delta() {
        let (rpc, mut received_requests, state, server) =
            start_failure_path_bitcoind(FailurePathConfig {
                transaction_in_mempool: false,
                mempool_check_fails: true,
                submission_behavior: SubmissionBehavior::Rejected,
                ambiguous_prioritize_response: false,
                rollback_fails: false,
            })
            .await;

        let error = rpc
            .update_transaction_priority(RAW_TX, true)
            .await
            .expect_err("mempool query failure should fail the update");

        assert!(matches!(
            error,
            super::PriorityUpdateError::OperationRolledBack {
                operation: super::PriorityUpdateOperation::CheckMempool,
                ..
            }
        ));
        assert_eq!(state.fee_delta.load(Ordering::SeqCst), 0);
        assert_request_methods(
            &mut received_requests,
            &[
                "getprioritisedtransactions",
                "prioritisetransaction",
                "getmempoolentry",
                "prioritisetransaction",
            ],
        )
        .await;

        server.abort();
    }

    #[tokio::test]
    async fn ambiguous_submission_error_is_success_when_reconciliation_finds_the_transaction() {
        let (rpc, mut received_requests, state, server) =
            start_failure_path_bitcoind(FailurePathConfig {
                transaction_in_mempool: false,
                mempool_check_fails: false,
                submission_behavior: SubmissionBehavior::AmbiguousApplied,
                ambiguous_prioritize_response: false,
                rollback_fails: false,
            })
            .await;

        rpc.update_transaction_priority(RAW_TX, true)
            .await
            .expect("mempool reconciliation should confirm submission");

        assert_eq!(state.fee_delta.load(Ordering::SeqCst), 100_000_000);
        assert_request_methods(
            &mut received_requests,
            &[
                "getprioritisedtransactions",
                "prioritisetransaction",
                "getmempoolentry",
                "sendrawtransaction",
                "getmempoolentry",
            ],
        )
        .await;

        server.abort();
    }

    #[tokio::test]
    async fn ambiguous_submission_error_rolls_back_when_reconciliation_finds_no_transaction() {
        let (rpc, mut received_requests, state, server) =
            start_failure_path_bitcoind(FailurePathConfig {
                transaction_in_mempool: false,
                mempool_check_fails: false,
                submission_behavior: SubmissionBehavior::AmbiguousNotApplied,
                ambiguous_prioritize_response: false,
                rollback_fails: false,
            })
            .await;

        let error = rpc
            .update_transaction_priority(RAW_TX, true)
            .await
            .expect_err("unconfirmed transaction submission should fail");

        assert!(matches!(
            error,
            super::PriorityUpdateError::OperationRolledBack {
                operation: super::PriorityUpdateOperation::Submit,
                ..
            }
        ));
        assert_eq!(state.fee_delta.load(Ordering::SeqCst), 0);
        assert_request_methods(
            &mut received_requests,
            &[
                "getprioritisedtransactions",
                "prioritisetransaction",
                "getmempoolentry",
                "sendrawtransaction",
                "getmempoolentry",
                "prioritisetransaction",
            ],
        )
        .await;

        server.abort();
    }

    #[tokio::test]
    async fn ambiguous_prioritization_error_is_reconciled_before_submission() {
        let (rpc, mut received_requests, state, server) =
            start_failure_path_bitcoind(FailurePathConfig {
                transaction_in_mempool: true,
                mempool_check_fails: false,
                submission_behavior: SubmissionBehavior::Rejected,
                ambiguous_prioritize_response: true,
                rollback_fails: false,
            })
            .await;

        rpc.update_transaction_priority(RAW_TX, true)
            .await
            .expect("fee delta reconciliation should confirm prioritization");

        assert_eq!(state.fee_delta.load(Ordering::SeqCst), 100_000_000);
        assert_request_methods(
            &mut received_requests,
            &[
                "getprioritisedtransactions",
                "prioritisetransaction",
                "getprioritisedtransactions",
                "getmempoolentry",
            ],
        )
        .await;

        server.abort();
    }

    #[tokio::test]
    async fn failed_rollback_reports_the_retained_delta() {
        let (rpc, mut received_requests, state, server) =
            start_failure_path_bitcoind(FailurePathConfig {
                transaction_in_mempool: false,
                mempool_check_fails: false,
                submission_behavior: SubmissionBehavior::Rejected,
                ambiguous_prioritize_response: false,
                rollback_fails: true,
            })
            .await;

        let error = rpc
            .update_transaction_priority(RAW_TX, true)
            .await
            .expect_err("submission and rollback failures should be reported");
        let error_message = error.to_string();

        assert!(matches!(
            error,
            super::PriorityUpdateError::RollbackFailed {
                operation: super::PriorityUpdateOperation::Submit,
                observed_delta: 100_000_000,
                ..
            }
        ));
        assert!(error_message.contains("priority rollback failed"));
        assert!(error_message.contains("fee delta 100000000"));
        assert_eq!(state.fee_delta.load(Ordering::SeqCst), 100_000_000);
        assert_request_methods(
            &mut received_requests,
            &[
                "getprioritisedtransactions",
                "prioritisetransaction",
                "getmempoolentry",
                "sendrawtransaction",
                "prioritisetransaction",
                "getprioritisedtransactions",
            ],
        )
        .await;

        server.abort();
    }

    #[tokio::test]
    async fn failed_deprioritization_returns_an_error_and_keeps_the_observed_delta() {
        let (rpc, mut received_requests, state, server) =
            start_failure_path_bitcoind(FailurePathConfig {
                transaction_in_mempool: true,
                mempool_check_fails: false,
                submission_behavior: SubmissionBehavior::Rejected,
                ambiguous_prioritize_response: false,
                rollback_fails: true,
            })
            .await;
        state.fee_delta.store(100_000_000, Ordering::SeqCst);

        let error = rpc
            .update_transaction_priority(RAW_TX, false)
            .await
            .expect_err("rejected deprioritization should fail");

        assert!(matches!(
            error,
            super::PriorityUpdateError::MutationFailed {
                operation: super::PriorityUpdateOperation::Deprioritize,
                observed_delta: Some(100_000_000),
                ..
            }
        ));
        assert_eq!(state.fee_delta.load(Ordering::SeqCst), 100_000_000);
        assert_request_methods(
            &mut received_requests,
            &["getprioritisedtransactions", "prioritisetransaction"],
        )
        .await;

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

    fn may_have_applied_mutation(&self) -> bool {
        matches!(
            self,
            BitcoindRpcError::Timeout(_)
                | BitcoindRpcError::Other(_)
                | BitcoindRpcError::InvalidResponse(_)
        )
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

#[derive(Clone, Copy, Debug)]
pub(crate) enum PriorityUpdateOperation {
    Prioritize,
    Deprioritize,
    CheckMempool,
    Submit,
}

impl fmt::Display for PriorityUpdateOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PriorityUpdateOperation::Prioritize => "prioritisetransaction",
            PriorityUpdateOperation::Deprioritize => "deprioritizing transaction",
            PriorityUpdateOperation::CheckMempool => "getmempoolentry",
            PriorityUpdateOperation::Submit => "sendrawtransaction",
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) enum PriorityUpdateError {
    Rpc(BitcoindRpcError),
    MutationFailed {
        operation: PriorityUpdateOperation,
        source: BitcoindRpcError,
        observed_delta: Option<i64>,
    },
    OperationFailed {
        operation: PriorityUpdateOperation,
        source: BitcoindRpcError,
    },
    OperationRolledBack {
        operation: PriorityUpdateOperation,
        source: BitcoindRpcError,
    },
    RollbackFailed {
        operation: PriorityUpdateOperation,
        source: BitcoindRpcError,
        rollback_error: BitcoindRpcError,
        observed_delta: i64,
    },
    StateIndeterminate {
        operation: PriorityUpdateOperation,
        source: BitcoindRpcError,
        details: String,
    },
}

impl PriorityUpdateError {
    pub(crate) fn status_code(&self) -> StatusCode {
        match self {
            PriorityUpdateError::Rpc(source)
            | PriorityUpdateError::MutationFailed { source, .. }
            | PriorityUpdateError::OperationFailed { source, .. }
            | PriorityUpdateError::OperationRolledBack { source, .. } => source.status_code(),
            PriorityUpdateError::RollbackFailed { .. }
            | PriorityUpdateError::StateIndeterminate { .. } => StatusCode::BAD_GATEWAY,
        }
    }
}

impl From<BitcoindRpcError> for PriorityUpdateError {
    fn from(error: BitcoindRpcError) -> Self {
        PriorityUpdateError::Rpc(error)
    }
}

impl fmt::Display for PriorityUpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PriorityUpdateError::Rpc(source) => source.fmt(f),
            PriorityUpdateError::MutationFailed {
                operation,
                source,
                observed_delta,
            } => {
                write!(f, "{operation} failed: {source}")?;
                if let Some(observed_delta) = observed_delta {
                    write!(
                        f,
                        "; reconciliation observed fee delta {observed_delta}"
                    )?;
                }
                Ok(())
            }
            PriorityUpdateError::OperationFailed { operation, source } => {
                write!(f, "{operation} failed: {source}")
            }
            PriorityUpdateError::OperationRolledBack { operation, source } => write!(
                f,
                "{operation} failed: {source}; the newly applied priority delta was rolled back"
            ),
            PriorityUpdateError::RollbackFailed {
                operation,
                source,
                rollback_error,
                observed_delta,
            } => write!(
                f,
                "{operation} failed: {source}; priority rollback failed: {rollback_error}; reconciliation observed fee delta {observed_delta}"
            ),
            PriorityUpdateError::StateIndeterminate {
                operation,
                source,
                details,
            } => write!(
                f,
                "{operation} failed ambiguously: {source}; final state is indeterminate: {details}"
            ),
        }
    }
}

#[derive(Debug)]
enum RollbackOutcome {
    Restored,
    Failed {
        rollback_error: BitcoindRpcError,
        observed_delta: i64,
    },
    Indeterminate {
        rollback_error: BitcoindRpcError,
        reconciliation_error: BitcoindRpcError,
    },
}
