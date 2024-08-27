use crate::translator::downstream::SUBSCRIBE_TIMEOUT_SECS;

use super::{downstream::Downstream, task_manager::TaskManager};
use roles_logic_sv2::utils::Mutex;
use std::sync::Arc;
use sv1_api::json_rpc;
use sv1_api::server_to_client;
use tokio::sync::broadcast;
use tokio::task;
use tracing::{error, warn};

pub async fn start_notify(
    task_manager: Arc<Mutex<TaskManager>>,
    downstream: Arc<Mutex<Downstream>>,
    mut rx_sv1_notify: broadcast::Receiver<server_to_client::Notify<'static>>,
    last_notify: Option<server_to_client::Notify<'static>>,
    host: String,
    connection_id: u32,
) -> Result<(), ()> {
    let handle = {
        let task_manager = task_manager.clone();
        task::spawn(async move {
            let timeout_timer = std::time::Instant::now();
            let mut first_sent = false;
            loop {
                let is_a = match downstream.safe_lock(|d| !d.authorized_names.is_empty()) {
                    Ok(is_a) => is_a,
                    Err(e) => {
                        error!("{}", e);
                        break;
                    }
                };
                if is_a && !first_sent && last_notify.is_some() {
                    Downstream::init_difficulty_management(&downstream)
                        .await
                        .unwrap();

                    let sv1_mining_notify_msg = last_notify.clone().unwrap();
                    let message: json_rpc::Message = sv1_mining_notify_msg.into();
                    Downstream::send_message_downstream(downstream.clone(), message)
                        .await
                        .unwrap();
                    if let Err(e) = downstream.clone().safe_lock(|s| {
                        s.first_job_received = true;
                    }) {
                        error!("{}", e);
                        break;
                    }
                    first_sent = true;
                } else if is_a && last_notify.is_some() {
                    if start_update(task_manager, downstream.clone())
                        .await
                        .is_err()
                    {
                        warn!("Translator impossible to start update task");
                        break;
                    };

                    while let Ok(sv1_mining_notify_msg) = rx_sv1_notify.recv().await {
                        downstream
                            .safe_lock(|d| d.last_notify = Some(sv1_mining_notify_msg.clone()))
                            .unwrap();
                        let message: json_rpc::Message = sv1_mining_notify_msg.into();
                        if Downstream::send_message_downstream(downstream.clone(), message)
                            .await
                            .is_err()
                        {
                            break;
                        };
                    }
                    break;
                } else {
                    // timeout connection if miner does not send the authorize message after sending a subscribe
                    if timeout_timer.elapsed().as_secs() > SUBSCRIBE_TIMEOUT_SECS {
                        warn!(
                            "Downstream: miner.subscribe/miner.authorize TIMEOUT for {} {}",
                            &host, connection_id
                        );
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
            // TODO here we want to be sure that on drop this is called
            let _ = Downstream::remove_downstream_hashrate_from_channel(&downstream);
            // TODO here we want to kill the tasks
            warn!(
                "Downstream: Shutting down sv1 downstream job notifier for {}",
                &host
            );
        })
    };
    TaskManager::add_notify(task_manager, handle.into()).await
}

async fn start_update(
    task_manager: Arc<Mutex<TaskManager>>,
    downstream: Arc<Mutex<Downstream>>,
) -> Result<(), ()> {
    let handle = task::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
            let ln = downstream.safe_lock(|d| d.last_notify.clone()).unwrap();
            assert!(ln.is_some());
            // if hashrate has changed, update difficulty management, and send new
            // mining.set_difficulty
            let _ = Downstream::try_update_difficulty_settings(&downstream, ln).await;
        }
    });
    TaskManager::add_update(task_manager, handle.into()).await
}
