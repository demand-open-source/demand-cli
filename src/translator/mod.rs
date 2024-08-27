mod downstream;

mod error;
mod proxy;
mod upstream;

use bitcoin::Address;

use roles_logic_sv2::{parsers::Mining, utils::Mutex};

use async_channel::bounded;
use std::{net::IpAddr, sync::Arc};
use tokio::sync::mpsc::channel;

use sv1_api::server_to_client;
use tokio::sync::broadcast;

use crate::shared::utils::AbortOnDrop;
use tokio::sync::mpsc::{Receiver as TReceiver, Sender as TSender};

use self::upstream::diff_management::UpstreamDifficultyConfig;
use roles_logic_sv2::parsers::PoolMessages;
mod task_manager;
use task_manager::TaskManager;

type PMessages = PoolMessages<'static>;

pub async fn start(
    downstreams: TReceiver<(TSender<String>, TReceiver<String>, IpAddr)>,
    pool_connection: TSender<(
        TSender<Mining<'static>>,
        TReceiver<Mining<'static>>,
        Option<Address>,
    )>,
) -> Result<AbortOnDrop, ()> {
    let task_manager = TaskManager::initialize(pool_connection.clone());
    let abortable = task_manager
        .safe_lock(|t| t.get_aborter())
        .unwrap()
        .unwrap();

    let (send_to_up, up_recv_from_here) = channel(crate::TRANSLATOR_BUFFER_SIZE);
    let (up_send_to_here, recv_from_up) = channel(crate::TRANSLATOR_BUFFER_SIZE);
    pool_connection
        .send((up_send_to_here, up_recv_from_here, None))
        .await
        .unwrap();

    // `tx_sv1_bridge` sender is used by `Downstream` to send a `DownstreamMessages` message to
    // `Bridge` via the `rx_sv1_downstream` receiver
    // (Sender<downstream_sv1::DownstreamMessages>, Receiver<downstream_sv1::DownstreamMessages>)
    let (tx_sv1_bridge, rx_sv1_downstream) = bounded(crate::TRANSLATOR_BUFFER_SIZE);

    // Sender/Receiver to send a SV2 `SubmitSharesExtended` from the `Bridge` to the `Upstream`
    // (Sender<SubmitSharesExtended<'static>>, Receiver<SubmitSharesExtended<'static>>)
    let (tx_sv2_submit_shares_ext, rx_sv2_submit_shares_ext) =
        bounded(crate::TRANSLATOR_BUFFER_SIZE);

    // Sender/Receiver to send a SV2 `SetNewPrevHash` message from the `Upstream` to the `Bridge`
    // (Sender<SetNewPrevHash<'static>>, Receiver<SetNewPrevHash<'static>>)
    let (tx_sv2_set_new_prev_hash, rx_sv2_set_new_prev_hash) =
        bounded(crate::TRANSLATOR_BUFFER_SIZE);

    // Sender/Receiver to send a SV2 `NewExtendedMiningJob` message from the `Upstream` to the
    // `Bridge`
    // (Sender<NewExtendedMiningJob<'static>>, Receiver<NewExtendedMiningJob<'static>>)
    let (tx_sv2_new_ext_mining_job, rx_sv2_new_ext_mining_job) =
        bounded(crate::TRANSLATOR_BUFFER_SIZE);

    // Sender/Receiver to send a new extranonce from the `Upstream` to this `main` function to be
    // passed to the `Downstream` upon a Downstream role connection
    // (Sender<ExtendedExtranonce>, Receiver<ExtendedExtranonce>)
    let (tx_sv2_extranonce, rx_sv2_extranonce) = bounded(crate::TRANSLATOR_BUFFER_SIZE);
    let target = Arc::new(Mutex::new(vec![0; 32]));

    // Sender/Receiver to send SV1 `mining.notify` message from the `Bridge` to the `Downstream`
    let (tx_sv1_notify, _): (
        broadcast::Sender<server_to_client::Notify>,
        broadcast::Receiver<server_to_client::Notify>,
    ) = broadcast::channel(crate::TRANSLATOR_BUFFER_SIZE);

    let upstream_diff = UpstreamDifficultyConfig {
        channel_diff_update_interval: crate::CHANNEL_DIFF_UPDTATE_INTERVAL,
        channel_nominal_hashrate: crate::EXPECTED_SV1_HASHPOWER,
    };
    let diff_config = Arc::new(Mutex::new(upstream_diff));

    // Instantiate a new `Upstream` (SV2 Pool)
    let upstream = match upstream::Upstream::new(
        rx_sv2_submit_shares_ext,
        tx_sv2_set_new_prev_hash,
        tx_sv2_new_ext_mining_job,
        crate::MIN_EXTRANONCE_SIZE,
        tx_sv2_extranonce,
        target.clone(),
        diff_config.clone(),
        send_to_up,
    )
    .await
    {
        Ok(upstream) => upstream,
        Err(_e) => {
            todo!();
        }
    };

    let upstream_abortable = upstream::Upstream::start(upstream, recv_from_up)
        .await
        .unwrap();

    let (extended_extranonce, up_id) = rx_sv2_extranonce.recv().await.unwrap();
    loop {
        let target: [u8; 32] = target.safe_lock(|t| t.clone()).unwrap().try_into().unwrap();
        if target != [0; 32] {
            break;
        };
        tokio::task::yield_now().await;
    }

    // Instantiate a new `Bridge` and begins handling incoming messages
    let b = proxy::Bridge::new(
        rx_sv1_downstream,
        tx_sv2_submit_shares_ext,
        rx_sv2_set_new_prev_hash,
        rx_sv2_new_ext_mining_job,
        tx_sv1_notify.clone(),
        extended_extranonce,
        target,
        up_id,
    );
    let bridge_aborter = proxy::Bridge::start(b.clone()).await?;

    let downstream_aborter = downstream::Downstream::accept_connections(
        tx_sv1_bridge,
        tx_sv1_notify,
        b,
        diff_config,
        downstreams,
    )
    .await?;

    TaskManager::add_bridge(task_manager.clone(), bridge_aborter).await?;
    TaskManager::add_downstream_listener(task_manager.clone(), downstream_aborter).await?;
    TaskManager::add_upstream(task_manager.clone(), upstream_abortable).await?;
    Ok(abortable)
}
