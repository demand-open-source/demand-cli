use super::{Downstream, DownstreamMessages, SetDownstreamTarget};
use binary_sv2::U256;
use roles_logic_sv2::{
    self,
    utils::{hash_rate_from_target, hash_rate_to_target},
};
use sv1_api::{self, methods::server_to_client::SetDifficulty, server_to_client::Notify};

use super::super::error::{Error, ProxyResult};
use roles_logic_sv2::utils::Mutex;
use std::{ops::Div, sync::Arc};
use sv1_api::json_rpc;

use bitcoin::util::uint::Uint256;
use tracing::{info, warn};

// TODO redesign it to not use all this mutexes
impl Downstream {
    /// Initializes difficult managment.
    /// Send downstream a first target.
    pub async fn init_difficulty_management(self_: &Arc<Mutex<Self>>) -> ProxyResult<()> {
        let timestamp_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time went backwards")
            .as_millis();

        self_
            .safe_lock(|d| {
                d.difficulty_mgmt.timestamp_of_last_update = timestamp_millis;
                d.difficulty_mgmt.submits_since_last_update = 0;
            })
            .map_err(|_e| Error::PoisonLock)?;

        let (message, _) = target_to_sv1_message(
            crate::EXPECTED_SV1_HASHPOWER.into(),
            crate::SHARE_PER_MIN.into(),
        )?;
        Downstream::send_message_downstream(self_.clone(), message).await?;

        Ok(())
    }

    /// Called before a miner disconnects so we can remove the miner's hashrate from the
    /// aggregated channel hashrate.
    pub fn remove_downstream_hashrate_from_channel(
        self_: &Arc<Mutex<Self>>,
    ) -> ProxyResult<'static, ()> {
        let (upstream_diff, estimated_downstream_hash_rate) = self_.safe_lock(|d| {
            (
                d.upstream_difficulty_config.clone(),
                d.difficulty_mgmt.estimated_downstream_hash_rate,
            )
        })?;
        info!(
            "Removing downstream hashrate from channel upstream_diff: {:?}, downstream_diff: {:?}",
            upstream_diff, estimated_downstream_hash_rate
        );
        upstream_diff.safe_lock(|u| {
            u.channel_nominal_hashrate -=
                // Make sure that upstream channel hasrate never goes below 0
                f32::min(estimated_downstream_hash_rate, u.channel_nominal_hashrate);
        })?;
        Ok(())
    }

    /// Calculate the new estimated downstream hashrate. And if is worth an update, update the
    /// downstream and the bridge. There is an hard limiti set by MIN_SV1_DOWSNTREAM_HASHRATE to
    /// avoid being flowed by millions of shares.
    pub async fn try_update_difficulty_settings(
        self_: &Arc<Mutex<Self>>,
        last_notify: Option<Notify<'static>>,
    ) -> ProxyResult<'static, ()> {
        let (down_diff_config, channel_id) = self_
            .clone()
            .safe_lock(|d| (d.difficulty_mgmt.clone(), d.connection_id))
            .map_err(|_e| Error::PoisonLock)?;

        let prev_target = match hash_rate_to_target(
            down_diff_config.estimated_downstream_hash_rate.into(),
            down_diff_config.shares_per_minute.into(),
        ) {
            Ok(target) => Ok(target),
            Err(v) => Err(Error::TargetError(v)),
        }?;

        if let Some(estimated_hash_rate) = Self::update_downstream_hashrate(self_, prev_target)? {
            let estimated_hash_rate =
                f32::max(estimated_hash_rate, crate::MIN_SV1_DOWSNTREAM_HASHRATE);
            Self::update_diff_setting(
                self_,
                down_diff_config.shares_per_minute,
                channel_id,
                estimated_hash_rate,
                last_notify,
            )
            .await?;
        }
        Ok(())
    }

    /// 1. Calculate the new target given the share per minute and the miner's hashrate.
    /// 2. Get difficulty from target and send it downstream also send the last mining.notify
    ///    sent so that downstream will hopefully immidiately update the target.
    /// 3. Updated the channel that the bridge use to mirror this downstream.
    async fn update_diff_setting(
        self_: &Arc<Mutex<Self>>,
        shares_per_minute: f32,
        channel_id: u32,
        estimated_hash_rate: f32,
        last_notify: Option<Notify<'static>>,
    ) -> ProxyResult<'static, ()> {
        // Send messages downstream
        let (message, target) =
            target_to_sv1_message(estimated_hash_rate.into(), shares_per_minute.into())?;
        Downstream::send_message_downstream(self_.clone(), message).await?;

        if let Some(notify) = last_notify {
            Downstream::send_message_downstream(self_.clone(), notify.into()).await?;
        }

        // Notify bridge of target update.
        let update_target_msg = SetDownstreamTarget {
            channel_id,
            new_target: target.into(),
        };
        Downstream::send_message_upstream(
            self_,
            DownstreamMessages::SetDownstreamTarget(update_target_msg),
        )
        .await?;
        Ok(())
    }

    /// Increments the number of shares since the last difficulty update.
    pub(super) fn save_share(self_: Arc<Mutex<Self>>) -> ProxyResult<'static, ()> {
        self_.safe_lock(|d| {
            d.difficulty_mgmt.submits_since_last_update += 1;
        })?;
        Ok(())
    }

    /// Convert a target into a `SetDifficulty` message.
    fn get_set_difficulty(target: [u8; 32]) -> json_rpc::Message {
        let value = Downstream::difficulty_from_target(target);
        let set_difficulty = SetDifficulty { value };
        let message: json_rpc::Message = set_difficulty.into();
        message
    }

    /// Convert a target into a dfficulty.
    fn difficulty_from_target(mut target: [u8; 32]) -> f64 {
        // Reverse because target is LE and this function relies on BE.
        target.reverse();

        // If received target is 0, return 0.
        if Downstream::is_zero(target.as_ref()) {
            warn!("Target is 0");
            return 0.0;
        }
        let target = Uint256::from_be_bytes(target);
        let pdiff: [u8; 32] = [
            0, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
            255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
        ];
        let pdiff = Uint256::from_be_bytes(pdiff);

        if pdiff > target {
            let diff = pdiff.div(target);
            diff.low_u64() as f64
        } else {
            let diff = target.div(pdiff);
            let diff = diff.low_u64() as f64;
            1.0 / diff
        }
    }

    /// Helper function to check if target is set to zero for some reason (typically happens when
    /// Downstream role first connects).
    /// https://stackoverflow.com/questions/65367552/checking-a-vecu8-to-see-if-its-all-zero
    fn is_zero(buf: &[u8]) -> bool {
        let (prefix, aligned, suffix) = unsafe { buf.align_to::<u128>() };

        prefix.iter().all(|&x| x == 0)
            && suffix.iter().all(|&x| x == 0)
            && aligned.iter().all(|&x| x == 0)
    }

    /// This function updates the miner hashrate and resets difficulty management params.
    /// To calculate hashrate it calculates the realized shares per minute from the number of
    /// shares submitted and the delta time since last update. It then uses the realized shares per
    /// minute and the target those shares where mined on to calculate an estimated hashrate during
    /// that period with the function [`roles_logic_sv2::utils::hash_rate_from_target`].
    /// Lastly, it adjusts the `channel_nominal_hashrate` according to the change in estimated
    /// miner hashrate
    pub fn update_downstream_hashrate(
        self_: &Arc<Mutex<Self>>,
        miner_target: U256<'static>,
    ) -> ProxyResult<'static, Option<f32>> {
        let timestamp_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time went backwards")
            .as_millis();
        let (difficulty_mgmt, last_call) =
            &self_.safe_lock(|d| (d.difficulty_mgmt.clone(), d.last_call_to_update_hr))?;
        let time_delta_millis = timestamp_millis - difficulty_mgmt.timestamp_of_last_update;

        // Check if last time that this fn have been called is less than 1 second ago.
        // If it is retunr Ok(None).
        // If it is not continue.
        if *last_call != 0 && (timestamp_millis - last_call) < 1000 {
            return Ok(None);
        }
        if time_delta_millis == 0 {
            return Ok(None);
        }
        self_.safe_lock(|d| d.last_call_to_update_hr = timestamp_millis)?;

        // This should have already been checked in the caller fn, no need to check it again
        assert!(difficulty_mgmt.timestamp_of_last_update != 0);

        let realized_share_per_min = difficulty_mgmt.submits_since_last_update as f64
            / (time_delta_millis as f64 / (60.0 * 1000.0));

        // We received too few shares in the interval to estimate a correct hashrate. If delta_time
        // is big enaugh we just try with last estimated hash rate divided by 1.5.
        if realized_share_per_min.is_nan() {
            panic!("realized_share_per_min should not be nan");
        } else if realized_share_per_min == 0.0 || realized_share_per_min.is_infinite() {
            if time_delta_millis < 5 * 1000 {
                Ok(None)
            } else {
                let new_estimation = difficulty_mgmt.estimated_downstream_hash_rate as f64 / 1.5;
                Self::update_self_with_new_hash_rate(
                    self_,
                    timestamp_millis,
                    new_estimation as f32,
                )?;
                Ok(Some(new_estimation as f32))
            }
        } else if realized_share_per_min.is_sign_negative() {
            panic!("realized_share_per_min should not be negative");
        } else {
            let new_estimation = hash_rate_from_target(miner_target, realized_share_per_min)?;

            if let Some(new_estimation) = Self::refine_new_estimation(
                time_delta_millis,
                difficulty_mgmt.estimated_downstream_hash_rate as f64,
                new_estimation,
            ) {
                Self::update_self_with_new_hash_rate(
                    self_,
                    timestamp_millis,
                    new_estimation as f32,
                )?;
                Ok(Some(new_estimation as f32))
            } else {
                Ok(None)
            }
        }
    }

    fn refine_new_estimation(
        time_delta_millis: u128,
        old_estimation: f64,
        new_estimation: f64,
    ) -> Option<f64> {
        let time_delata_secs = time_delta_millis / 1000;

        let hashrate_delta = new_estimation - old_estimation;
        let hashrate_delta_percentage = (hashrate_delta.abs() / old_estimation) * 100.0;

        if (hashrate_delta_percentage >= 80.0)
            || (hashrate_delta_percentage >= 60.0) && (time_delata_secs >= 60)
            || (hashrate_delta_percentage >= 50.0) && (time_delata_secs >= 120)
            || (hashrate_delta_percentage >= 45.0) && (time_delata_secs >= 180)
            || (hashrate_delta_percentage >= 30.0) && (time_delata_secs >= 240)
            || (hashrate_delta_percentage >= 15.0) && (time_delata_secs >= 300)
        {
            if hashrate_delta_percentage > 1000.0 {
                let new_estimation = match time_delata_secs {
                    dt if dt <= 30 => old_estimation * 10.0,
                    dt if dt < 60 => old_estimation * 5.0,
                    _ => old_estimation * 3.0,
                };
                Some(new_estimation)
            } else {
                Some(new_estimation)
            }
        } else {
            None
        }
    }
    fn update_self_with_new_hash_rate(
        self_: &Arc<Mutex<Self>>,
        now: u128,
        new_estimation: f32,
    ) -> ProxyResult<'static, ()> {
        let (upstream_difficulty_config, old_estimation) = self_.safe_lock(|d| {
            let old_estimation = d.difficulty_mgmt.estimated_downstream_hash_rate;
            d.difficulty_mgmt.estimated_downstream_hash_rate = new_estimation;
            d.difficulty_mgmt.timestamp_of_last_update = now;
            d.difficulty_mgmt.submits_since_last_update = 0;
            (d.upstream_difficulty_config.clone(), old_estimation)
        })?;
        let hash_rate_delta = new_estimation - old_estimation;
        upstream_difficulty_config.safe_lock(|c| {
            if c.channel_nominal_hashrate + hash_rate_delta > 0.0 {
                c.channel_nominal_hashrate += hash_rate_delta;
            } else {
                c.channel_nominal_hashrate = 0.0;
            }
        })?;
        Ok(())
    }
}

fn target_to_sv1_message(
    hash_power: f64,
    share_per_min: f64,
) -> ProxyResult<'static, (json_rpc::Message, [u8; 32])> {
    let target = match hash_rate_to_target(hash_power, share_per_min) {
        Ok(target) => Ok(target),
        Err(v) => Err(Error::TargetError(v)),
    }?;
    let target: [u8; 32] = target
        .inner_as_ref()
        .try_into()
        .expect("Should always be 32 bytes");
    Ok((Downstream::get_set_difficulty(target), target))
}

#[cfg(test)]
mod test {
    use super::super::super::upstream::diff_management::UpstreamDifficultyConfig;
    use crate::translator::downstream::{downstream::DownstreamDifficultyConfig, Downstream};
    use async_channel::unbounded;
    use binary_sv2::U256;
    use rand::{thread_rng, Rng};
    use roles_logic_sv2::{mining_sv2::Target, utils::Mutex};
    use sha2::{Digest, Sha256};
    use std::{
        sync::Arc,
        time::{Duration, Instant},
    };
    use tokio::sync::mpsc::channel;

    #[test]
    fn test_diff_management() {
        let expected_shares_per_minute = 1000.0;
        let total_run_time = std::time::Duration::from_secs(40);
        let initial_nominal_hashrate = dbg!(measure_hashrate(10));
        let target = match roles_logic_sv2::utils::hash_rate_to_target(
            initial_nominal_hashrate,
            expected_shares_per_minute.into(),
        ) {
            Ok(target) => target,
            Err(_) => panic!(),
        };

        let mut share = generate_random_80_byte_array();
        let timer = std::time::Instant::now();
        let mut elapsed = std::time::Duration::from_secs(0);
        let mut count = 0;
        while elapsed <= total_run_time {
            // start hashing util a target is met and submit to
            mock_mine(target.clone().into(), &mut share);
            elapsed = timer.elapsed();
            count += 1;
        }

        let calculated_share_per_min = count as f32 / (elapsed.as_secs_f32() / 60.0);
        // This is the error margin for a confidence of 99% given the expect number of shares per
        // minute TODO the review the math under it
        let error_margin = get_error(expected_shares_per_minute.into());
        let error =
            (dbg!(calculated_share_per_min) - dbg!(expected_shares_per_minute as f32)).abs();
        assert!(
            dbg!(error) <= error_margin as f32,
            "Calculated shares per minute are outside the 99% confidence interval. Error: {:?}, Error margin: {:?}, {:?}", error, error_margin,calculated_share_per_min
        );
    }

    fn get_error(lambda: f64) -> f64 {
        let z_score_99 = 2.576;
        z_score_99 * lambda.sqrt()
    }

    fn mock_mine(target: Target, share: &mut [u8; 80]) {
        let mut hashed: Target = [255_u8; 32].into();
        while hashed > target {
            hashed = hash(share);
        }
    }

    // returns hashrate based on how fast the device hashes over the given duration
    fn measure_hashrate(duration_secs: u64) -> f64 {
        let mut share = generate_random_80_byte_array();
        let start_time = Instant::now();
        let mut hashes: u64 = 0;
        let duration = Duration::from_secs(duration_secs);

        while start_time.elapsed() < duration {
            for _ in 0..10000 {
                hash(&mut share);
                hashes += 1;
            }
        }

        let elapsed_secs = start_time.elapsed().as_secs_f64();
        let hashrate = hashes as f64 / elapsed_secs;
        let nominal_hash_rate = hashrate;
        nominal_hash_rate
    }

    fn hash(share: &mut [u8; 80]) -> Target {
        let nonce: [u8; 8] = share[0..8].try_into().unwrap();
        let mut nonce = u64::from_le_bytes(nonce);
        nonce += 1;
        share[0..8].copy_from_slice(&nonce.to_le_bytes());
        let hash = Sha256::digest(&share).to_vec();
        let hash: U256<'static> = hash.try_into().unwrap();
        hash.into()
    }

    fn generate_random_80_byte_array() -> [u8; 80] {
        let mut rng = thread_rng();
        let mut arr = [0u8; 80];
        rng.fill(&mut arr[..]);
        arr
    }

    #[tokio::test]
    async fn test_converge_to_spm_from_low() {
        test_converge_to_spm(1.0).await
    }

    #[tokio::test]
    async fn test_converge_to_spm_from_high() {
        // TODO make this converge in acceptable times also for bigger numbers
        test_converge_to_spm(500_000.0).await
    }

    async fn test_converge_to_spm(start_hashrate: f64) {
        let downstream_conf = DownstreamDifficultyConfig {
            estimated_downstream_hash_rate: 0.0, // updated below
            shares_per_minute: 1000.0,           // 1000 shares per minute
            submits_since_last_update: 0,
            timestamp_of_last_update: 0, // updated below
        };
        let upstream_config = UpstreamDifficultyConfig {
            channel_diff_update_interval: 60,
            channel_nominal_hashrate: 0.0,
            timestamp_of_last_update: 0,
            should_aggregate: false,
        };
        let (tx_sv1_submit, _rx_sv1_submit) = unbounded();
        let (tx_outgoing, _rx_outgoing) = channel(10);
        let mut downstream = Downstream::new(
            1,
            vec![],
            vec![],
            None,
            None,
            tx_sv1_submit,
            tx_outgoing,
            false,
            0,
            downstream_conf.clone(),
            Arc::new(Mutex::new(upstream_config)),
        );
        downstream.difficulty_mgmt.estimated_downstream_hash_rate = start_hashrate as f32;

        let total_run_time = std::time::Duration::from_secs(10);
        let config_shares_per_minute = downstream_conf.shares_per_minute;
        let timer = std::time::Instant::now();
        let mut elapsed = std::time::Duration::from_secs(0);

        let expected_nominal_hashrate = measure_hashrate(5);
        let expected_target = match roles_logic_sv2::utils::hash_rate_to_target(
            expected_nominal_hashrate,
            config_shares_per_minute.into(),
        ) {
            Ok(target) => target,
            Err(_) => panic!(),
        };

        let initial_nominal_hashrate = start_hashrate;
        let mut initial_target = match roles_logic_sv2::utils::hash_rate_to_target(
            initial_nominal_hashrate,
            config_shares_per_minute.into(),
        ) {
            Ok(target) => target,
            Err(_) => panic!(),
        };
        let downstream = Arc::new(Mutex::new(downstream));
        Downstream::init_difficulty_management(&downstream)
            .await
            .unwrap();
        let mut share = generate_random_80_byte_array();
        while elapsed <= total_run_time {
            mock_mine(initial_target.clone().into(), &mut share);
            Downstream::save_share(downstream.clone()).unwrap();
            let _ = Downstream::try_update_difficulty_settings(&downstream, None).await;
            initial_target = downstream
                .safe_lock(|d| {
                    match roles_logic_sv2::utils::hash_rate_to_target(
                        d.difficulty_mgmt.estimated_downstream_hash_rate.into(),
                        config_shares_per_minute.into(),
                    ) {
                        Ok(target) => target,
                        Err(_) => panic!(),
                    }
                })
                .unwrap();
            elapsed = timer.elapsed();
        }
        let expected_0s = trailing_0s(expected_target.inner_as_ref().to_vec());
        let actual_0s = trailing_0s(initial_target.inner_as_ref().to_vec());
        assert!(expected_0s.abs_diff(actual_0s) <= 1);
    }
    fn trailing_0s(mut v: Vec<u8>) -> usize {
        let mut ret = 0;
        while v.pop() == Some(0) {
            ret += 1;
        }
        ret
    }
    // TODO make a test where unknown donwstream is simulated and we do not wait for it to produce
    // a share but we try to updated the estimated hash power every 2 seconds and updated the
    // target consequentially this shuold start to provide shares within a normal amount of time
}
