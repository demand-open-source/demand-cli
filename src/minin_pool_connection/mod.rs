mod task_manager;

use std::net::SocketAddr;

use codec_sv2::{HandshakeRole, StandardEitherFrame, StandardSv2Frame};
use demand_share_accounting_ext::parser::PoolExtMessages;
use demand_sv2_connection::noise_connection_tokio::Connection;
use key_utils::Secp256k1PublicKey;
use noise_sv2::Initiator;
use rand::distributions::{Alphanumeric, DistString};
use roles_logic_sv2::{
    common_messages_sv2::{Protocol, SetupConnection, SetupConnectionSuccess},
    parsers::CommonMessages,
};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{Receiver, Sender};
use tracing::{error, info};

use crate::shared::utils::AbortOnDrop;
use task_manager::TaskManager;

pub type Message = PoolExtMessages<'static>;
pub type StdFrame = StandardSv2Frame<Message>;
pub type EitherFrame = StandardEitherFrame<Message>;

const DEFAULT_TIMER: std::time::Duration = std::time::Duration::from_secs(5);

pub async fn connect_pool(
    address: SocketAddr,
    authority_public_key: Secp256k1PublicKey,
    setup_connection_msg: Option<SetupConnection<'static>>,
    timer: Option<std::time::Duration>,
) -> Result<
    (
        Sender<PoolExtMessages<'static>>,
        Receiver<PoolExtMessages<'static>>,
        AbortOnDrop,
    ),
    (),
> {
    let socket = loop {
        match TcpStream::connect(address).await {
            Ok(socket) => break socket,
            Err(e) => {
                error!(
                    "Failed to connect to Upstream role at {}, retrying in 5s: {}",
                    address, e
                );
                tokio::time::sleep(std::time::Duration::from_secs(5)).await
            }
        }
    };

    let initiator =
        Initiator::from_raw_k(authority_public_key.into_bytes()).expect("Invalid authority key");

    info!(
        "PROXY SERVER - ACCEPTING FROM UPSTREAM: {}",
        socket.peer_addr().expect("Failed to get peer address")
    );

    // Channel to send and receive messages to the SV2 Upstream role
    let (mut receiver, mut sender, _, _) =
        Connection::new(socket, HandshakeRole::Initiator(initiator))
            .await
            .expect("Failed to create connection");
    let setup_connection_msg =
        setup_connection_msg.unwrap_or(get_mining_setup_connection_msg(true));
    if let Ok(_) = mining_setup_connection(
        &mut receiver,
        &mut sender,
        setup_connection_msg,
        timer.unwrap_or(DEFAULT_TIMER),
    )
    .await
    {
        let task_manager = TaskManager::initialize();
        let abortable = task_manager
            .safe_lock(|t| t.get_aborter())
            .unwrap()
            .unwrap();

        let (send_to_down, recv_from_down) = tokio::sync::mpsc::channel(10);
        let (send_from_down, recv_to_up) = tokio::sync::mpsc::channel(10);
        let relay_up_task = relay_up(recv_to_up, sender);
        TaskManager::add_sv2_relay_up(task_manager.clone(), relay_up_task)
            .await
            .expect("Task Manager failed");
        let relay_down_task = relay_down(receiver, send_to_down);
        TaskManager::add_sv2_relay_down(task_manager.clone(), relay_down_task)
            .await
            .expect("Task Manager failed");
        Ok((send_from_down, recv_from_down, abortable))
    } else {
        Err(())
    }
}

fn relay_up(
    mut recv: Receiver<PoolExtMessages<'static>>,
    send: Sender<EitherFrame>,
) -> AbortOnDrop {
    let task = tokio::spawn(async move {
        while let Some(msg) = recv.recv().await {
            let std_frame: Result<StdFrame, _> = msg.try_into();
            if let Ok(std_frame) = std_frame {
                let either_frame: EitherFrame = std_frame.into();
                if send.send(either_frame).await.is_err() {
                    error!("Mining upstream failed");
                    break;
                };
            } else {
                panic!("Internal Mining downstream try to send invalid message");
            }
        }
    });
    task.into()
}

fn relay_down(
    mut recv: Receiver<EitherFrame>,
    send: Sender<PoolExtMessages<'static>>,
) -> AbortOnDrop {
    let task = tokio::spawn(async move {
        while let Some(msg) = recv.recv().await {
            let msg: Result<StdFrame, ()> = msg.try_into().map_err(|_| ());
            if let Ok(mut msg) = msg {
                if let Some(header) = msg.get_header() {
                    let message_type = header.msg_type();
                    let payload = msg.payload();
                    let extension = header.ext_type();
                    let msg: Result<PoolExtMessages<'_>, _> =
                        (extension, message_type, payload).try_into();
                    if let Ok(msg) = msg {
                        let msg = msg.into_static();
                        if send.send(msg).await.is_err() {
                            panic!("Internal Mining downstream not available");
                        }
                    } else {
                        error!("Mining Upstream send non Mining message. Disconnecting");
                        break;
                    }
                } else {
                    error!("Mining Upstream send invalid message no header. Disconnecting");
                    break;
                }
            } else {
                error!("Mining Upstream down.");
                break;
            }
        }
    });
    task.into()
}

async fn mining_setup_connection(
    recv: &mut Receiver<EitherFrame>,
    send: &mut Sender<EitherFrame>,
    setup_conection: SetupConnection<'static>,
    timer: std::time::Duration,
) -> Result<SetupConnectionSuccess, ()> {
    let msg = PoolExtMessages::Common(CommonMessages::SetupConnection(setup_conection));
    let std_frame: StdFrame = msg.try_into().unwrap();
    let either_frame: EitherFrame = std_frame.into();
    send.send(either_frame).await.unwrap();
    if let Ok(Some(msg)) = tokio::time::timeout(timer, recv.recv()).await {
        let mut msg: StdFrame = msg.try_into().map_err(|_| ())?;
        let header = msg.get_header().ok_or(())?;
        let message_type = header.msg_type();
        let payload = msg.payload();
        let msg: CommonMessages<'_> = (message_type, payload).try_into().unwrap();
        match msg {
            CommonMessages::SetupConnectionSuccess(s) => Ok(s),
            _ => return Err(()),
        }
    } else {
        Err(())
    }
}

pub fn get_mining_setup_connection_msg(work_selection: bool) -> SetupConnection<'static> {
    let endpoint_host = "0.0.0.0".to_string().into_bytes().try_into().unwrap();
    let vendor = String::new().try_into().unwrap();
    let hardware_version = String::new().try_into().unwrap();
    let firmware = String::new().try_into().unwrap();
    let flags = match work_selection {
        false => 0b0000_0000_0000_0000_0000_0000_0000_0100,
        true => 0b0000_0000_0000_0000_0000_0000_0000_0110,
    };
    let address = std::env::var("ADDRESS").unwrap();
    let device_id = Alphanumeric.sample_string(&mut rand::thread_rng(), 16);
    let device_id = format!("{}::SOLO::{}", device_id, address)
        .to_string()
        .try_into()
        .unwrap();
    SetupConnection {
        protocol: Protocol::MiningProtocol,
        min_version: 2,
        max_version: 2,
        flags,
        endpoint_host,
        endpoint_port: 50,
        vendor,
        hardware_version,
        firmware,
        device_id,
    }
}
