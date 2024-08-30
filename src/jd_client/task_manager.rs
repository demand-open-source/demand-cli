use std::sync::Arc;

use crate::shared::utils::AbortOnDrop;
use roles_logic_sv2::utils::Mutex;
use tokio::sync::mpsc;
use tracing::warn;

#[derive(Debug)]
#[allow(dead_code)]
enum Task {
    JobDeclarator(AbortOnDrop),
    TemplateReceiver(AbortOnDrop),
    MiningUpstream(AbortOnDrop),
    MiningDowntream(AbortOnDrop),
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

    pub async fn add_job_declarator_task(
        self_: Arc<Mutex<Self>>,
        abortable: AbortOnDrop,
    ) -> Result<(), ()> {
        let send_task = self_.safe_lock(|s| s.send_task.clone()).unwrap();
        send_task
            .send(Task::JobDeclarator(abortable))
            .await
            .map_err(|_| ())
    }
    pub async fn add_template_receiver_task(
        self_: Arc<Mutex<Self>>,
        abortable: AbortOnDrop,
    ) -> Result<(), ()> {
        let send_task = self_.safe_lock(|s| s.send_task.clone()).unwrap();
        send_task
            .send(Task::TemplateReceiver(abortable))
            .await
            .map_err(|_| ())
    }
    pub async fn add_mining_upstream_task(
        self_: Arc<Mutex<Self>>,
        abortable: AbortOnDrop,
    ) -> Result<(), ()> {
        let send_task = self_.safe_lock(|s| s.send_task.clone()).unwrap();
        send_task
            .send(Task::MiningUpstream(abortable))
            .await
            .map_err(|_| ())
    }
    pub async fn add_mining_downtream_task(
        self_: Arc<Mutex<Self>>,
        abortable: AbortOnDrop,
    ) -> Result<(), ()> {
        let send_task = self_.safe_lock(|s| s.send_task.clone()).unwrap();
        send_task
            .send(Task::MiningDowntream(abortable))
            .await
            .map_err(|_| ())
    }
}
