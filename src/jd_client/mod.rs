#![allow(special_module_name)]

pub mod downstream;
mod job_declarator;
mod proxy_config;
mod template_receiver;
mod upstream_sv2;
mod error;

use downstream::DownstreamMiningNode;
use job_declarator::JobDeclarator;
use proxy_config::ProxyConfig;
use template_receiver::TemplateRx;
use std::sync::atomic::AtomicBool;

/// Is used by the template receiver and the downstream. When a NewTemplate is received the context
/// that is running the template receiver set this value to false and then the message is sent to
/// the context that is running the Downstream that do something and then set it back to true.
///
/// In the meantime if the context that is running the template receiver receives a SetNewPrevHash
/// it wait until the value of this global is true before doing anything.
///
/// Acuire and Release memory ordering is used.
///
/// Memory Ordering Explanation:
/// We use Acquire-Release ordering instead of SeqCst or Relaxed for the following reasons:
/// 1. Acquire in template receiver context ensures we see all operations before the Release store
///    the downstream.
/// 2. Within the same execution context (template receiver), a Relaxed store followed by an Acquire
///    load is sufficient. This is because operations within the same context execute in the order
///    they appear in the code.
/// 3. The combination of Release in downstream and Acquire in template receiver contexts establishes
///    a happens-before relationship, guaranteeing that we handle the SetNewPrevHash message after
///    that downstream have finished handling the NewTemplate.
/// 3. SeqCst is overkill we only need to synchronize two contexts, a globally agreed-upon order
///    between all the contexts is not necessary.
pub static IS_NEW_TEMPLATE_HANDLED: AtomicBool = AtomicBool::new(true);

#[derive(Debug)]
pub struct PoolChangerTrigger {
    #[allow(unused)]
    timeout: Duration,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl PoolChangerTrigger {
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            task: None,
        }
    }

    pub fn stop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

use roles_logic_sv2::{parsers::Mining, utils::Mutex};
use std::{
    net::{IpAddr, SocketAddr},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use std::net::ToSocketAddrs;
use tracing::error;

pub async fn start(
    receiver: tokio::sync::mpsc::Receiver<Mining<'static>>,
    sender: tokio::sync::mpsc::Sender<Mining<'static>>,
    up_receiver: tokio::sync::mpsc::Receiver<Mining<'static>>,
    up_sender: tokio::sync::mpsc::Sender<Mining<'static>>,
) {
    if std::env::var("ADDRESS").is_err() {
        error!("ADDRESS env variable not set");
        std::process::exit(1);
    }

    let upstream_index = 0;

    let proxy_config = ProxyConfig::default();

    if let Some(upstream) = proxy_config.upstreams.get(upstream_index) {
        let initialize = initialize_jd(
            upstream.clone(),
            proxy_config.timeout,
            receiver,
            sender,
            up_receiver,
            up_sender,
        );
        tokio::task::spawn(initialize);
    } else {
        let initialize = initialize_jd_as_solo_miner(proxy_config.timeout, receiver, sender);
        tokio::task::spawn(initialize);
    }
}
async fn initialize_jd_as_solo_miner(
    timeout: Duration,
    receiver: tokio::sync::mpsc::Receiver<Mining<'static>>,
    sender: tokio::sync::mpsc::Sender<Mining<'static>>,
) {
    let proxy_config = ProxyConfig::default();
    let miner_tx_out =
        proxy_config::get_coinbase_output(std::env::var("ADDRESS").unwrap().as_str()).unwrap();

    // When Downstream receive a share that meets bitcoin target it transformit in a
    // SubmitSolution and send it to the TemplateReceiver
    let (send_solution, recv_solution) = tokio::sync::mpsc::channel(10);

    let donwstream = Arc::new(Mutex::new(downstream::DownstreamMiningNode::new(
        sender,
        None,
        send_solution,
        proxy_config.withhold,
        miner_tx_out.clone(),
        None,
    )));
    DownstreamMiningNode::start(donwstream.clone(), receiver);

    // Initialize JD part
    let mut parts = proxy_config.tp_address.split(':');
    let ip_tp = parts.next().unwrap().to_string();
    let port_tp = parts.next().unwrap().parse::<u16>().unwrap();

    TemplateRx::connect(
        SocketAddr::new(IpAddr::from_str(ip_tp.as_str()).unwrap(), port_tp),
        recv_solution,
        None,
        donwstream,
        Arc::new(Mutex::new(PoolChangerTrigger::new(timeout))),
        miner_tx_out.clone(),
        proxy_config.tp_authority_public_key,
        false,
    )
    .await;
}

async fn initialize_jd(
    upstream_config: proxy_config::Upstream,
    timeout: Duration,
    receiver: tokio::sync::mpsc::Receiver<Mining<'static>>,
    sender: tokio::sync::mpsc::Sender<Mining<'static>>,
    up_receiver: tokio::sync::mpsc::Receiver<Mining<'static>>,
    up_sender: tokio::sync::mpsc::Sender<Mining<'static>>,
) {
    let proxy_config = ProxyConfig::default();
    let test_only_do_not_send_solution_to_tp = proxy_config
        .test_only_do_not_send_solution_to_tp
        .unwrap_or(false);

    // When Downstream receive a share that meets bitcoin target it transformit in a
    // SubmitSolution and send it to the TemplateReceiver
    let (send_solution, recv_solution) = tokio::sync::mpsc::channel(10);

    // Instantiate a new `Upstream` (SV2 Pool)
    let upstream = match upstream_sv2::Upstream::new(
        0, // TODO
        upstream_config.pool_signature.clone(),
        Arc::new(Mutex::new(PoolChangerTrigger::new(timeout))),
        up_sender,
    )
    .await
    {
        Ok(upstream) => upstream,
        Err(e) => {
            error!("Failed to create upstream: {}", e);
            panic!()
        }
    };

    // Start receiving messages from the SV2 Upstream role
    if let Err(e) = upstream_sv2::Upstream::parse_incoming(upstream.clone(), up_receiver) {
        error!("failed to create sv2 parser: {}", e);
        panic!()
    }

    // Initialize JD part
    let mut parts = proxy_config.tp_address.split(':');
    let ip_tp = parts.next().unwrap().to_string();
    let port_tp = parts.next().unwrap().parse::<u16>().unwrap();

    let jd = match JobDeclarator::new(
        upstream_config
            .jd_address
            .clone()
            .to_socket_addrs()
            .unwrap()
            .next()
            .unwrap(),
        upstream_config.authority_pubkey.into_bytes(),
        proxy_config.clone(),
        upstream.clone(),
    )
    .await
    {
        Ok(c) => c,
        Err(_e) => {
            // TODO TODO TODO
            todo!()
        }
    };

    let donwstream = Arc::new(Mutex::new(downstream::DownstreamMiningNode::new(
        sender,
        Some(upstream.clone()),
        send_solution,
        proxy_config.withhold,
        vec![],
        Some(jd.clone()),
    )));
    DownstreamMiningNode::start(donwstream.clone(), receiver);

    TemplateRx::connect(
        SocketAddr::new(IpAddr::from_str(ip_tp.as_str()).unwrap(), port_tp),
        recv_solution,
        Some(jd.clone()),
        donwstream,
        Arc::new(Mutex::new(PoolChangerTrigger::new(timeout))),
        vec![],
        proxy_config.tp_authority_public_key,
        test_only_do_not_send_solution_to_tp,
    )
    .await;
}
