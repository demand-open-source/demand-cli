use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        OnceLock,
    },
    time::Instant,
};
use tokio::sync::{mpsc, oneshot};
use tracing::debug;

static COUNTING_SINCE: OnceLock<Instant> = OnceLock::new();

static SENT_BYTES: AtomicU64 = AtomicU64::new(0);
static RECEIVED_BYTES: AtomicU64 = AtomicU64::new(0);
/// Last declaration round trip in ms; 0 means none yet.
static DECLARATION_MS: AtomicU64 = AtomicU64::new(0);

fn counting_since() -> &'static Instant {
    COUNTING_SINCE.get_or_init(Instant::now)
}

pub fn record_sent(bytes: usize) {
    counting_since();
    SENT_BYTES.fetch_add(bytes as u64, Relaxed);
}

pub fn record_received(bytes: usize) {
    counting_since();
    RECEIVED_BYTES.fetch_add(bytes as u64, Relaxed);
}

pub fn record_declaration_latency(millis: u64) {
    DECLARATION_MS.store(millis.max(1), Relaxed);
}

/// Returns the average bandwidth in bytes per second since the first record_sent or record_received call.
pub fn bandwidth_bytes_per_sec() -> Option<u64> {
    let elapsed = COUNTING_SINCE.get()?.elapsed().as_secs();
    if elapsed == 0 {
        return None;
    }
    Some((SENT_BYTES.load(Relaxed) + RECEIVED_BYTES.load(Relaxed)) / elapsed)
}

pub fn declaration_latency_ms() -> Option<u64> {
    match DECLARATION_MS.load(Relaxed) {
        0 => None,
        millis => Some(millis),
    }
}

#[derive(Debug)]
enum StatsCommand {
    SetupStats(u32),
    UpdateHashrate(u32, f32),
    UpdateDiff(u32, f32),
    UpdateAcceptedShares(u32),
    UpdateRejectedShares(u32),
    UpdateDeviceName(u32, String),
    RemoveStats(u32),
    GetStats(oneshot::Sender<HashMap<u32, DownstreamConnectionStats>>),
}

#[derive(Debug, Clone, Serialize)]
pub struct DownstreamConnectionStats {
    pub device_name: Option<String>,
    pub hashrate: f32,
    pub accepted_shares: u64,
    pub rejected_shares: u64,
    pub current_difficulty: f32,
}

impl DownstreamConnectionStats {
    fn new() -> Self {
        Self {
            device_name: None,
            hashrate: 0.0,
            accepted_shares: 0,
            rejected_shares: 0,
            current_difficulty: 0.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StatsSender {
    sender: mpsc::Sender<StatsCommand>,
}

impl StatsSender {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(100);
        tokio::spawn(StatsManager::new(rx).run());
        Self { sender: tx }
    }

    fn send(&self, command: StatsCommand) {
        if let Err(e) = self.sender.try_send(command) {
            debug!("Failed to send stats command: {:?}", e);
        }
    }

    async fn send_reliable(&self, command: StatsCommand) -> Result<(), String> {
        self.sender.send(command).await.map_err(|e| e.to_string())
    }

    pub async fn setup_stats_reliable(&self, connection_id: u32) -> Result<(), String> {
        self.send_reliable(StatsCommand::SetupStats(connection_id))
            .await
    }

    pub fn update_hashrate(&self, connection_id: u32, hashrate: f32) {
        self.send(StatsCommand::UpdateHashrate(connection_id, hashrate));
    }

    pub fn update_diff(&self, connection_id: u32, diff: f32) {
        self.send(StatsCommand::UpdateDiff(connection_id, diff));
    }

    pub fn update_accepted_shares(&self, connection_id: u32) {
        self.send(StatsCommand::UpdateAcceptedShares(connection_id));
    }

    pub fn update_rejected_shares(&self, connection_id: u32) {
        self.send(StatsCommand::UpdateRejectedShares(connection_id));
    }

    pub fn update_device_name(&self, connection_id: u32, name: String) {
        self.send(StatsCommand::UpdateDeviceName(connection_id, name));
    }

    pub async fn remove_stats_reliable(&self, connection_id: u32) -> Result<(), String> {
        self.send_reliable(StatsCommand::RemoveStats(connection_id))
            .await
    }

    pub async fn collect_stats(&self) -> Result<HashMap<u32, DownstreamConnectionStats>, String> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(StatsCommand::GetStats(tx))
            .await
            .map_err(|e| e.to_string())?;
        match rx.await {
            Ok(stats) => Ok(stats),
            Err(e) => Err(e.to_string()),
        }
    }
}

struct StatsManager {
    stats: HashMap<u32, DownstreamConnectionStats>,
    receiver: mpsc::Receiver<StatsCommand>,
}

impl StatsManager {
    fn new(receiver: mpsc::Receiver<StatsCommand>) -> Self {
        Self {
            stats: HashMap::new(),
            receiver,
        }
    }

    async fn run(mut self) {
        while let Some(msg) = self.receiver.recv().await {
            match msg {
                StatsCommand::SetupStats(id) => {
                    self.stats.insert(id, DownstreamConnectionStats::new());
                }
                StatsCommand::UpdateHashrate(id, hashrate) => {
                    if let Some(stats) = self.stats.get_mut(&id) {
                        stats.hashrate = hashrate
                    }
                }
                StatsCommand::UpdateDiff(id, diff) => {
                    if let Some(stats) = self.stats.get_mut(&id) {
                        stats.current_difficulty = diff
                    }
                }
                StatsCommand::UpdateAcceptedShares(id) => {
                    if let Some(stats) = self.stats.get_mut(&id) {
                        stats.accepted_shares += 1
                    }
                }
                StatsCommand::UpdateRejectedShares(id) => {
                    if let Some(stats) = self.stats.get_mut(&id) {
                        stats.rejected_shares += 1
                    }
                }
                StatsCommand::UpdateDeviceName(id, name) => {
                    if let Some(stats) = self.stats.get_mut(&id) {
                        stats.device_name = Some(name)
                    }
                }
                StatsCommand::RemoveStats(id) => {
                    self.stats.remove(&id);
                }
                StatsCommand::GetStats(tx) => {
                    let _ = tx.send(self.stats.clone());
                }
            }
        }
    }
}
