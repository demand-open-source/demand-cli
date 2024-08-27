use super::task_manager::TaskManager;
use roles_logic_sv2::utils::Mutex;
use std::sync::Arc;
use sv1_api::json_rpc;
use tokio::sync::mpsc;
use tokio::task;
use tracing::warn;

pub async fn start_send_to_downstream(
    task_manager: Arc<Mutex<TaskManager>>,
    mut receiver_outgoing: mpsc::Receiver<json_rpc::Message>,
    send_to_down: mpsc::Sender<String>,
    connection_id: u32,
    host: String,
) -> Result<(), ()> {
    let handle = task::spawn(async move {
        while let Some(res) = receiver_outgoing.recv().await {
            let to_send = match serde_json::to_string(&res) {
                Ok(string) => format!("{}\n", string),
                Err(_e) => {
                    break;
                }
            };
            match send_to_down.send(to_send).await {
                Ok(_) => (),
                Err(_e) => {
                    warn!("Downstream {} dropped", host);
                    break;
                }
            }
        }
        warn!(
            "Downstream: Shutting down sv1 downstream writer: {}",
            connection_id
        );
    });
    TaskManager::add_send_downstream(task_manager, handle.into()).await
}
