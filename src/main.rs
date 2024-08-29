use jemallocator::Jemalloc;
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

use key_utils::Secp256k1PublicKey;
//use std::time::Duration;
//use key_utils::Secp256k1PublicKey;
use lazy_static::lazy_static;
use std::net::ToSocketAddrs;
use tokio::sync::mpsc::channel;

mod ingress;
pub mod jd_client;
mod minin_pool_connection;
mod share_accounter;
mod shared;
mod translator;

const TRANSLATOR_BUFFER_SIZE: usize = 32;
const MIN_EXTRANONCE_SIZE: u16 = 8;
//const EXPECTED_SV1_HASHPOWER: f32 = 5_000_000_000_000.0;
const EXPECTED_SV1_HASHPOWER: f32 = 1_000_000.0;
const SHARE_PER_MIN: f32 = 10.0;
const CHANNEL_DIFF_UPDTATE_INTERVAL: u32 = 10;
//const MIN_SV1_DOWSNTREAM_HASHRATE: f32 = 1_000_000_000_000.0;
const MIN_SV1_DOWSNTREAM_HASHRATE: f32 = 1_000_000.0;
const POOL_SIGNATURE: &str = "DEMAND";
const MAX_LEN_DOWN_MSG: u32 = 10000;
const POOL_ADDRESS: &str = "mining.dmnd.work:2000";
const AUTH_PUB_KEY: &str = "9bQHWXsQ2J9TRFTaxRh3KjoxdyLRfWVEy25YHtKF8y8gotLoCZZ";
const TP_ADDRESS: &str = "127.0.0.1:8432";

lazy_static! {
    static ref SV1_DOWN_LISTEN_ADDR: String = std::env::var("SV1_DOWN_LISTEN_ADDR")
        .expect("SV1_DOWN_LISTEN_ADDR env variable is not set");
}

#[tokio::main]
async fn main() {
    let auth_pub_k: Secp256k1PublicKey = crate::AUTH_PUB_KEY.parse().expect("Invalid public key");
    let address = POOL_ADDRESS
        .to_socket_addrs()
        .expect("Invalid pool address")
        .next()
        .expect("Invalid pool address");
    let (send_to_pool, recv_from_pool, _) =
        minin_pool_connection::connect_pool(address, auth_pub_k, None, None)
            .await
            .expect("Impossible connect to the pool");

    let (downs_sv1_tx, downs_sv1_rx) = channel(10);
    ingress::sv1_ingress::start(downs_sv1_tx).await;

    let (translator_up_tx, mut translator_up_rx) = channel(10);
    let _ = translator::start(downs_sv1_rx, translator_up_tx)
        .await
        .expect("Impossible initialize translator");

    let (from_jdc_to_share_accounter_send, from_jdc_to_share_accounter_recv) = channel(10);
    let (from_share_accounter_to_jdc_send, from_share_accounter_to_jdc_recv) = channel(10);
    let (jdc_to_translator_sender, jdc_from_translator_receiver, _) = translator_up_rx
        .recv()
        .await
        .expect("translator failed before initialization");
    jd_client::start(
        jdc_from_translator_receiver,
        jdc_to_translator_sender,
        from_share_accounter_to_jdc_recv,
        from_jdc_to_share_accounter_send,
    )
    .await;

    let _ = share_accounter::start(
        from_jdc_to_share_accounter_recv,
        from_share_accounter_to_jdc_send,
        recv_from_pool,
        send_to_pool,
    )
    .await;
}
