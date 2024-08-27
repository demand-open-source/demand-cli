use super::{downstream::Downstream, task_manager::TaskManager};
use roles_logic_sv2::utils::Mutex;
use std::sync::Arc;
use sv1_api::{client_to_server::Submit, json_rpc};
use tokio::sync::mpsc;
use tokio::task;
use tracing::{error, warn};

pub async fn start_receive_downstream(
    task_manager: Arc<Mutex<TaskManager>>,
    downstream: Arc<Mutex<Downstream>>,
    mut recv_from_down: mpsc::Receiver<String>,
    connection_id: u32,
) -> Result<(), ()> {
    let handle = task::spawn(async move {
        while let Some(incoming) = recv_from_down.recv().await {
            let incoming: Result<json_rpc::Message, _> = serde_json::from_str(&incoming);
            if let Ok(incoming) = incoming {
                // if message is Submit Shares update difficulty management
                if let sv1_api::Message::StandardRequest(standard_req) = incoming.clone() {
                    if let Ok(Submit { .. }) = standard_req.try_into() {
                        if let Err(e) = Downstream::save_share(downstream.clone()) {
                            error!("{}", e);
                            break;
                        }
                    }
                }
                // TODO handle panic
                Downstream::handle_incoming_sv1(downstream.clone(), incoming)
                    .await
                    .unwrap();
            } else {
                break;
            }
        }
        warn!(
            "Downstream: Shutting down sv1 downstream reader {}",
            connection_id
        );
    });
    TaskManager::add_receive_downstream(task_manager, handle.into()).await
}
