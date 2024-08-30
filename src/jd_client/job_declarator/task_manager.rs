use std::sync::Arc;

use crate::shared::utils::AbortOnDrop;
use roles_logic_sv2::utils::Mutex;
use tokio::sync::mpsc;
use tracing::warn;

#[derive(Debug)]
#[allow(dead_code)]
enum Task {
    AllocateTokens(AbortOnDrop),
    #[allow(clippy::enum_variant_names)]
    MainTask(AbortOnDrop),
    OnSetNewPrevHash(AbortOnDrop),
}

#[derive(Debug)]
pub struct TaskManager {
    send_task: mpsc::Sender<Task>,
    abort: Option<AbortOnDrop>,
}

impl TaskManager {
    #[allow(unused_variables)]
    pub fn initialize() -> Arc<Mutex<Self>> {
        let (sender, mut receiver) = mpsc::channel(10);
        let handle = tokio::task::spawn(async move {
            let mut tasks = vec![];
            while let Some(task) = receiver.recv().await {
                tasks.push(task);
            }
            warn!("Share accounter main task manager stopped, keep alive tasks");
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

    pub async fn add_allocate_tokens(
        self_: Arc<Mutex<Self>>,
        abortable: AbortOnDrop,
    ) -> Result<(), ()> {
        let send_task = self_.safe_lock(|s| s.send_task.clone()).unwrap();
        send_task
            .send(Task::AllocateTokens(abortable))
            .await
            .map_err(|_| ())
    }
    pub async fn add_main_task(self_: Arc<Mutex<Self>>, abortable: AbortOnDrop) -> Result<(), ()> {
        let send_task = self_.safe_lock(|s| s.send_task.clone()).unwrap();
        send_task
            .send(Task::MainTask(abortable))
            .await
            .map_err(|_| ())
    }
    pub async fn add_on_set_new_prev_hash(
        self_: Arc<Mutex<Self>>,
        abortable: AbortOnDrop,
    ) -> Result<(), ()> {
        let send_task = self_.safe_lock(|s| s.send_task.clone()).unwrap();
        send_task
            .send(Task::OnSetNewPrevHash(abortable))
            .await
            .map_err(|_| ())
    }
}
