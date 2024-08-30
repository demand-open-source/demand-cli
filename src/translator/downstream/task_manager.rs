use std::sync::Arc;

use crate::shared::utils::AbortOnDrop;
use roles_logic_sv2::utils::Mutex;
use tokio::sync::mpsc;
use tracing::warn;

#[allow(dead_code)]
enum Task {
    AcceptConnection(AbortOnDrop),
    ReceiveDownstream(AbortOnDrop),
    SendDownstream(AbortOnDrop),
    Notify(AbortOnDrop),
    Update(AbortOnDrop),
}

pub struct TaskManager {
    send_task: mpsc::Sender<Task>,
    abort: Option<AbortOnDrop>,
}

impl TaskManager {
    pub fn initialize() -> Arc<Mutex<Self>> {
        let (sender, mut receiver) = mpsc::channel(10);
        let handle = tokio::task::spawn(async move {
            let mut tasks = vec![];
            while let Some(task) = receiver.recv().await {
                tasks.push(task);
            }
            warn!("Translator downstream task manager stopped, keep alive tasks");
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1000)).await;
            }
        });
        Arc::new(Mutex::new(Self {
            send_task: sender,
            abort: Some(handle.into()),
        }))
    }

    pub fn get_aborter(&mut self) -> Option<AbortOnDrop> {
        self.abort.take()
    }

    pub async fn add_receive_downstream(
        self_: Arc<Mutex<Self>>,
        abortable: AbortOnDrop,
    ) -> Result<(), ()> {
        let send_task = self_.safe_lock(|s| s.send_task.clone()).unwrap();
        send_task
            .send(Task::ReceiveDownstream(abortable))
            .await
            .map_err(|_| ())
    }
    pub async fn add_update(self_: Arc<Mutex<Self>>, abortable: AbortOnDrop) -> Result<(), ()> {
        let send_task = self_.safe_lock(|s| s.send_task.clone()).unwrap();
        send_task
            .send(Task::Update(abortable))
            .await
            .map_err(|_| ())
    }
    pub async fn add_notify(self_: Arc<Mutex<Self>>, abortable: AbortOnDrop) -> Result<(), ()> {
        let send_task = self_.safe_lock(|s| s.send_task.clone()).unwrap();
        send_task
            .send(Task::Notify(abortable))
            .await
            .map_err(|_| ())
    }
    pub async fn add_send_downstream(
        self_: Arc<Mutex<Self>>,
        abortable: AbortOnDrop,
    ) -> Result<(), ()> {
        let send_task = self_.safe_lock(|s| s.send_task.clone()).unwrap();
        send_task
            .send(Task::SendDownstream(abortable))
            .await
            .map_err(|_| ())
    }
    pub async fn add_accept_connection(
        self_: Arc<Mutex<Self>>,
        abortable: AbortOnDrop,
    ) -> Result<(), ()> {
        let send_task = self_.safe_lock(|s| s.send_task.clone()).unwrap();
        send_task
            .send(Task::AcceptConnection(abortable))
            .await
            .map_err(|_| ())
    }
}
