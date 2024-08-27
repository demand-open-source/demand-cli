use super::Upstream;

#[derive(Debug, Clone)]
pub struct UpstreamDifficultyConfig {
    pub channel_diff_update_interval: u32,
    pub channel_nominal_hashrate: f32,
}

use super::super::error::{Error::PoisonLock, ProxyResult};
use binary_sv2::u256_from_int;
use roles_logic_sv2::{
    mining_sv2::UpdateChannel, parsers::Mining, utils::Mutex, Error as RolesLogicError,
};
use std::{sync::Arc, time::Duration};

impl Upstream {
    /// this function checks if the elapsed time since the last update has surpassed the config
    pub(super) async fn try_update_hashrate(self_: Arc<Mutex<Self>>) -> ProxyResult<'static, ()> {
        let (channel_id_option, diff_mgmt, tx_message) = self_
            .safe_lock(|u| (u.channel_id, u.difficulty_config.clone(), u.sender.clone()))
            .map_err(|_e| PoisonLock)?;
        let channel_id = channel_id_option.ok_or(super::super::error::Error::RolesSv2Logic(
            RolesLogicError::NotFoundChannelId,
        ))?;
        let (timeout, new_hashrate) = diff_mgmt
            .safe_lock(|d| (d.channel_diff_update_interval, d.channel_nominal_hashrate))
            .map_err(|_e| PoisonLock)?;
        // UPDATE CHANNEL
        let update_channel = UpdateChannel {
            channel_id,
            nominal_hash_rate: new_hashrate,
            maximum_target: u256_from_int(u64::MAX),
        };
        let message = Mining::UpdateChannel(update_channel);

        tx_message.send(message).await.unwrap();
        tokio::time::sleep(Duration::from_secs(timeout as u64)).await;
        Ok(())
    }
}
