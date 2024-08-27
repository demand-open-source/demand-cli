use std::sync::Arc;

use crate::shared::utils::AbortOnDrop;
use bitcoin::Address;
use roles_logic_sv2::parsers::Mining;
use roles_logic_sv2::utils::Mutex;
use tokio::sync::mpsc;
use tokio::sync::mpsc::{Receiver, Sender};
use tracing::warn;

type Message = Mining<'static>;

enum Task {
    DownstreamListener(AbortOnDrop),
    Bridge(AbortOnDrop),
    Upstream(AbortOnDrop),
}

pub struct TaskManager {
    send_task: mpsc::Sender<Task>,
    abort: Option<AbortOnDrop>,
}

impl TaskManager {
    #[allow(unused_variables)]
    pub fn initialize(
        // We need this to be alive for all the live of the translator
        up_connection: Sender<(Sender<Message>, Receiver<Message>, Option<Address>)>,
    ) -> Arc<Mutex<Self>> {
        let (sender, mut receiver) = mpsc::channel(10);
        let handle = tokio::task::spawn(async move {
            let mut tasks = vec![];
            while let Some(task) = receiver.recv().await {
                tasks.push(task);
            }
            warn!("Translator main task manager stopped, keep alive tasks");
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1000)).await;
            }
            #[allow(unreachable_code)]
            drop(up_connection)
        });
        Arc::new(Mutex::new(Self {
            send_task: sender,
            abort: Some(handle.into()),
        }))
    }

    pub fn get_aborter(&mut self) -> Option<AbortOnDrop> {
        self.abort.take()
    }

    pub async fn add_downstream_listener(
        self_: Arc<Mutex<Self>>,
        abortable: AbortOnDrop,
    ) -> Result<(), ()> {
        let send_task = self_.safe_lock(|s| s.send_task.clone()).unwrap();
        send_task
            .send(Task::DownstreamListener(abortable))
            .await
            .map_err(|_| ())
    }
    pub async fn add_upstream(self_: Arc<Mutex<Self>>, abortable: AbortOnDrop) -> Result<(), ()> {
        let send_task = self_.safe_lock(|s| s.send_task.clone()).unwrap();
        send_task
            .send(Task::Upstream(abortable))
            .await
            .map_err(|_| ())
    }
    pub async fn add_bridge(self_: Arc<Mutex<Self>>, abortable: AbortOnDrop) -> Result<(), ()> {
        let send_task = self_.safe_lock(|s| s.send_task.clone()).unwrap();
        send_task
            .send(Task::Bridge(abortable))
            .await
            .map_err(|_| ())
    }
}
