pub(crate) mod bitcoin_rpc;
mod routes;
pub mod stats;
mod utils;
use std::sync::Arc;

use crate::{api::bitcoin_rpc::BitcoindRpc, router::Router, Configuration};
use axum::{
    routing::{get, post},
    Router as AxumRouter,
};
use routes::Api;
use stats::StatsSender;
use tracing::{error, info};

// Holds shared state (like the router) that so that it can be accessed in all routes.
#[derive(Clone)]
pub struct AppState {
    router: Router,
    stats_sender: StatsSender,
    downstream_handoff: crate::DownstreamHandoffSender,
    prioritizing_txs: Option<PrioritizingTxs>,
}

#[derive(Clone)]
struct PrioritizingTxs {
    rpc: Arc<BitcoindRpc>,
    api_tx_token: String,
}

pub(crate) async fn start(
    router: Router,
    stats_sender: StatsSender,
    downstream_handoff: crate::DownstreamHandoffSender,
) {
    let prioritizing_txs = Configuration::bitcoind_rpc_config().map(|config| {
        let rpc = Arc::new(BitcoindRpc::new(
            config.url,
            config.user,
            config.pwd,
            config.fee_delta,
        ));
        PrioritizingTxs {
            rpc,
            api_tx_token: config.api_tx_token,
        }
    });

    let state = AppState {
        router,
        stats_sender,
        downstream_handoff,
        prioritizing_txs,
    };
    let app = AxumRouter::new()
        .route("/api/health", get(Api::health_check))
        .route(
            "/api/coinbase/op-return",
            post(crate::merge_mining::set_pair_api),
        )
        .route(
            "/api/merge-mining/found-job",
            get(crate::merge_mining::poll_found_job_api),
        )
        .route("/api/tx/submit/{tx}", post(Api::send_tx_to_bitcoind))
        .route(
            "/api/tx/prioritized",
            get(Api::get_prioritized_transactions),
        )
        .route("/api/pool/info", get(Api::get_pool_info))
        .route("/api/stats/miners", get(Api::get_downstream_stats))
        .route("/api/stats/aggregate", get(Api::get_aggregate_stats))
        .route("/api/stats/session-timing", get(Api::get_session_timing))
        .route("/api/stats/system", get(Api::system_stats))
        .with_state(state);

    let api_server_port = crate::config::Configuration::api_server_port();
    let api_bind_address =
        std::env::var("API_BIND_ADDRESS").unwrap_or_else(|_| "0.0.0.0".to_string());
    let api_server_addr = format!("{api_bind_address}:{api_server_port}");
    loop {
        let listener = match tokio::net::TcpListener::bind(&api_server_addr).await {
            Ok(listener) => listener,
            Err(error) => {
                error!(%error, %api_server_addr, "API server could not bind; mining remains active");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
        };
        info!(%api_server_addr, "API server listening");
        if let Err(error) = axum::serve(listener, app.clone()).await {
            error!(%error, "API server stopped; mining remains active");
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
