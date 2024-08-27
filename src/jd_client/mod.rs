#![allow(special_module_name)]

pub mod lib;

use lib::{
    downstream::DownstreamMiningNode, job_declarator::JobDeclarator, proxy_config::ProxyConfig,
    template_receiver::TemplateRx, PoolChangerTrigger,
};

use async_channel::unbounded;
use futures::{select, FutureExt};
use roles_logic_sv2::{
    parsers::{Mining, PoolMessages},
    utils::Mutex,
};
use std::{
    net::{IpAddr, SocketAddr},
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use tokio::task::AbortHandle;

pub use crate::status;
use std::net::ToSocketAddrs;
use tracing::{error, info};

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
    let mut interrupt_signal_future = Box::pin(tokio::signal::ctrl_c().fuse());

    // Channel used to manage failed tasks
    let (tx_status, rx_status) = unbounded();

    let task_collector = Arc::new(Mutex::new(vec![]));

    let proxy_config = ProxyConfig::default();

    {
        let task_collector = task_collector.clone();
        let tx_status = tx_status.clone();

        if let Some(upstream) = proxy_config.upstreams.get(upstream_index) {
            let initialize = initialize_jd(
                tx_status.clone(),
                task_collector,
                upstream.clone(),
                proxy_config.timeout,
                receiver,
                sender,
                up_receiver,
                up_sender,
            );
            tokio::task::spawn(initialize);
        } else {
            let initialize = initialize_jd_as_solo_miner(
                tx_status.clone(),
                task_collector,
                proxy_config.timeout,
                receiver,
                sender,
            );
            tokio::task::spawn(initialize);
        }
    }
    // Check all tasks if is_finished() is true, if so exit
    loop {
        let task_status = select! {
            task_status = rx_status.recv().fuse() => task_status,
            interrupt_signal = interrupt_signal_future => {
                match interrupt_signal {
                    Ok(()) => {
                        info!("Interrupt received");
                    },
                    Err(err) => {
                        error!("Unable to listen for interrupt signal: {}", err);
                        // we also shut down in case of error
                    },
                }
                std::process::exit(0);
            }
        };
        let task_status: status::Status = task_status.unwrap();

        match task_status.state {
            // Should only be sent by the downstream listener
            status::State::DownstreamShutdown(err) => {
                error!("SHUTDOWN from: {}", err);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                task_collector
                    .safe_lock(|s| {
                        for handle in s {
                            handle.abort();
                        }
                    })
                    .unwrap();
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                break;
            }
            status::State::UpstreamShutdown(err) => {
                error!("SHUTDOWN from: {}", err);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                task_collector
                    .safe_lock(|s| {
                        for handle in s {
                            handle.abort();
                        }
                    })
                    .unwrap();
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                break;
            }
            status::State::UpstreamRogue => {
                error!("Changin Pool");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                task_collector
                    .safe_lock(|s| {
                        for handle in s {
                            handle.abort();
                        }
                    })
                    .unwrap();
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                break;
            }
            status::State::Healthy(msg) => {
                info!("HEALTHY message: {}", msg);
            }
        }
    }
}
async fn initialize_jd_as_solo_miner(
    tx_status: async_channel::Sender<status::Status<'static>>,
    task_collector: Arc<Mutex<Vec<AbortHandle>>>,
    timeout: Duration,
    receiver: tokio::sync::mpsc::Receiver<Mining<'static>>,
    sender: tokio::sync::mpsc::Sender<Mining<'static>>,
) {
    let proxy_config = ProxyConfig::default();
    let miner_tx_out =
        lib::proxy_config::get_coinbase_output(std::env::var("ADDRESS").unwrap().as_str()).unwrap();

    // When Downstream receive a share that meets bitcoin target it transformit in a
    // SubmitSolution and send it to the TemplateReceiver
    let (send_solution, recv_solution) = tokio::sync::mpsc::channel(10);

    let donwstream = Arc::new(Mutex::new(lib::downstream::DownstreamMiningNode::new(
        sender,
        None,
        send_solution,
        proxy_config.withhold,
        task_collector.clone(),
        status::Sender::Downstream(tx_status.clone()),
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
        status::Sender::TemplateReceiver(tx_status.clone()),
        None,
        donwstream,
        task_collector,
        Arc::new(Mutex::new(PoolChangerTrigger::new(timeout))),
        miner_tx_out.clone(),
        proxy_config.tp_authority_public_key,
        false,
    )
    .await;
}

async fn initialize_jd(
    tx_status: async_channel::Sender<status::Status<'static>>,
    task_collector: Arc<Mutex<Vec<AbortHandle>>>,
    upstream_config: lib::proxy_config::Upstream,
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
    let upstream = match lib::upstream_sv2::Upstream::new(
        0, // TODO
        upstream_config.pool_signature.clone(),
        status::Sender::Upstream(tx_status.clone()),
        task_collector.clone(),
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
    if let Err(e) = lib::upstream_sv2::Upstream::parse_incoming(upstream.clone(), up_receiver) {
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
        task_collector.clone(),
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            let _ = tx_status
                .send(status::Status {
                    state: status::State::UpstreamShutdown(e),
                })
                .await;
            return;
        }
    };

    let donwstream = Arc::new(Mutex::new(lib::downstream::DownstreamMiningNode::new(
        sender,
        Some(upstream.clone()),
        send_solution,
        proxy_config.withhold,
        task_collector.clone(),
        status::Sender::Downstream(tx_status.clone()),
        vec![],
        Some(jd.clone()),
    )));
    DownstreamMiningNode::start(donwstream.clone(), receiver);

    TemplateRx::connect(
        SocketAddr::new(IpAddr::from_str(ip_tp.as_str()).unwrap(), port_tp),
        recv_solution,
        status::Sender::TemplateReceiver(tx_status.clone()),
        Some(jd.clone()),
        donwstream,
        task_collector,
        Arc::new(Mutex::new(PoolChangerTrigger::new(timeout))),
        vec![],
        proxy_config.tp_authority_public_key,
        test_only_do_not_send_solution_to_tp,
    )
    .await;
}
