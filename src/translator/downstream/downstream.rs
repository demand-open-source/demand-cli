use crate::{shared::utils::AbortOnDrop, translator::error::Error};

use super::{
    super::{error::ProxyResult, upstream::diff_management::UpstreamDifficultyConfig},
    task_manager::TaskManager,
};
use tokio::sync::{
    broadcast,
    mpsc::{channel, error::SendError, Receiver, Sender},
};

use super::{
    accept_connection::start_accept_connection, notify::start_notify,
    receive_from_downstream::start_receive_downstream,
    send_to_downstream::start_send_to_downstream, DownstreamMessages, SubmitShareWithChannelId,
};

use roles_logic_sv2::{
    common_properties::{IsDownstream, IsMiningDownstream},
    utils::Mutex,
};

use std::{net::IpAddr, sync::Arc};
use sv1_api::{
    client_to_server, json_rpc, server_to_client,
    utils::{Extranonce, HexU32Be},
    IsServer,
};
use tracing::{error, info, warn};

#[derive(Debug, Clone)]
pub struct DownstreamDifficultyConfig {
    pub estimated_downstream_hash_rate: f32,
    pub shares_per_minute: f32,
    pub submits_since_last_update: u32,
    pub timestamp_of_last_update: u128,
}

impl PartialEq for DownstreamDifficultyConfig {
    fn eq(&self, other: &Self) -> bool {
        other.estimated_downstream_hash_rate.round() as u32
            == self.estimated_downstream_hash_rate.round() as u32
    }
}

/// Handles the sending and receiving of messages to and from an SV2 Upstream role (most typically
/// a SV2 Pool server).
#[derive(Debug)]
pub struct Downstream {
    /// List of authorized Downstream Mining Devices.
    pub(super) connection_id: u32,
    pub(super) authorized_names: Vec<String>,
    extranonce1: Vec<u8>,
    /// `extranonce1` to be sent to the Downstream in the SV1 `mining.subscribe` message response.
    //extranonce1: Vec<u8>,
    //extranonce2_size: usize,
    /// Version rolling mask bits
    version_rolling_mask: Option<HexU32Be>,
    /// Minimum version rolling mask bits size
    version_rolling_min_bit: Option<HexU32Be>,
    /// Sends a SV1 `mining.submit` message received from the Downstream role to the `Bridge` for
    /// translation into a SV2 `SubmitSharesExtended`.
    tx_sv1_bridge: Sender<DownstreamMessages>,
    tx_outgoing: Sender<json_rpc::Message>,
    /// True if this is the first job received from `Upstream`.
    pub(super) first_job_received: bool,
    extranonce2_len: usize,
    pub(super) difficulty_mgmt: DownstreamDifficultyConfig,
    pub(super) upstream_difficulty_config: Arc<Mutex<UpstreamDifficultyConfig>>,
    pub last_call_to_update_hr: u128,
    pub(super) last_notify: Option<server_to_client::Notify<'static>>,
}

impl Downstream {
    /// Instantiate a new `Downstream`.
    #[allow(clippy::too_many_arguments)]
    pub async fn new_downstream(
        connection_id: u32,
        tx_sv1_bridge: Sender<DownstreamMessages>,
        rx_sv1_notify: broadcast::Receiver<server_to_client::Notify<'static>>,
        extranonce1: Vec<u8>,
        last_notify: Option<server_to_client::Notify<'static>>,
        extranonce2_len: usize,
        host: String,
        upstream_difficulty_config: Arc<Mutex<UpstreamDifficultyConfig>>,
        send_to_down: Sender<String>,
        recv_from_down: Receiver<String>,
        task_manager: Arc<Mutex<TaskManager>>,
    ) {
        assert!(last_notify.is_some());

        let (tx_outgoing, receiver_outgoing) = channel(crate::TRANSLATOR_BUFFER_SIZE);
        let difficulty_mgmt = DownstreamDifficultyConfig {
            estimated_downstream_hash_rate: crate::EXPECTED_SV1_HASHPOWER,
            shares_per_minute: crate::SHARE_PER_MIN,
            submits_since_last_update: 0,
            timestamp_of_last_update: 0,
        };

        let downstream = Arc::new(Mutex::new(Downstream {
            connection_id,
            authorized_names: vec![],
            extranonce1,
            version_rolling_mask: None,
            version_rolling_min_bit: None,
            tx_sv1_bridge,
            tx_outgoing,
            first_job_received: false,
            extranonce2_len,
            difficulty_mgmt,
            upstream_difficulty_config,
            last_call_to_update_hr: 0,
            last_notify: last_notify.clone(),
        }));

        // TODO handle error
        start_receive_downstream(
            task_manager.clone(),
            downstream.clone(),
            recv_from_down,
            connection_id,
        )
        .await
        .unwrap();

        // TODO handle error
        start_send_to_downstream(
            task_manager.clone(),
            receiver_outgoing,
            send_to_down,
            connection_id,
            host.clone(),
        )
        .await
        .unwrap();

        start_notify(
            task_manager.clone(),
            downstream.clone(),
            rx_sv1_notify,
            last_notify,
            host.clone(),
            connection_id,
        )
        .await
        .unwrap();
    }

    /// Accept connections from one or more SV1 Downstream roles (SV1 Mining Devices) and create a
    /// new `Downstream` for each connection.
    pub async fn accept_connections(
        tx_sv1_submit: Sender<DownstreamMessages>,
        tx_mining_notify: broadcast::Sender<server_to_client::Notify<'static>>,
        bridge: Arc<Mutex<super::super::proxy::Bridge>>,
        upstream_difficulty_config: Arc<Mutex<UpstreamDifficultyConfig>>,
        downstreams: Receiver<(Sender<String>, Receiver<String>, IpAddr)>,
    ) -> Result<AbortOnDrop, ()> {
        let task_manager = TaskManager::initialize();
        let abortable = task_manager
            .safe_lock(|t| t.get_aborter())
            .unwrap()
            .unwrap();
        start_accept_connection(
            task_manager.clone(),
            tx_sv1_submit,
            tx_mining_notify,
            bridge,
            upstream_difficulty_config,
            downstreams,
        )
        .await?;
        Ok(abortable)
    }

    /// As SV1 messages come in, determines if the message response needs to be translated to SV2
    /// and sent to the `Upstream`, or if a direct response can be sent back by the `Translator`
    /// (SV1 and SV2 protocol messages are NOT 1-to-1).
    pub(super) async fn handle_incoming_sv1(
        self_: Arc<Mutex<Self>>,
        message_sv1: json_rpc::Message,
    ) -> Result<(), super::super::error::Error<'static>> {
        // `handle_message` in `IsServer` trait + calls `handle_request`
        // TODO: Map err from V1Error to Error::V1Error

        let response = self_
            .safe_lock(|s| s.handle_message(message_sv1.clone()))
            .unwrap();
        match response {
            Ok(res) => {
                if let Some(r) = res {
                    // If some response is received, indicates no messages translation is needed
                    // and response should be sent directly to the SV1 Downstream. Otherwise,
                    // message will be sent to the upstream Translator to be translated to SV2 and
                    // forwarded to the `Upstream`
                    // let sender = self_.safe_lock(|s| s.connection.sender_upstream)
                    if let Err(e) = Self::send_message_downstream(self_, r.into()).await {
                        error!("{}", e);
                        todo!();
                    }
                    Ok(())
                } else {
                    // If None response is received, indicates this SV1 message received from the
                    // Downstream MD is passed to the `Translator` for translation into SV2
                    Ok(())
                }
            }
            // TODO
            Err(e) => {
                error!("{}", e);
                todo!()
            }
        }
    }

    /// Send SV1 response message that is generated by `Downstream` (as opposed to being received
    /// by `Bridge`) to be written to the SV1 Downstream role.
    pub(super) async fn send_message_downstream(
        self_: Arc<Mutex<Self>>,
        response: json_rpc::Message,
    ) -> Result<(), SendError<sv1_api::Message>> {
        let sender = self_.safe_lock(|s| s.tx_outgoing.clone()).unwrap();
        sender.send(response).await
    }

    /// Send SV1 response message that is generated by `Downstream` (as opposed to being received
    /// by `Bridge`) to be written to the SV1 Downstream role.
    pub(super) async fn send_message_upstream(
        self_: &Arc<Mutex<Self>>,
        msg: DownstreamMessages,
    ) -> ProxyResult<'static, ()> {
        let sender = self_
            .safe_lock(|s| s.tx_sv1_bridge.clone())
            .map_err(|_| Error::PoisonLock)?;
        let _ = sender.send(msg).await;
        Ok(())
    }
    #[cfg(test)]
    pub fn new(
        connection_id: u32,
        authorized_names: Vec<String>,
        extranonce1: Vec<u8>,
        version_rolling_mask: Option<HexU32Be>,
        version_rolling_min_bit: Option<HexU32Be>,
        tx_sv1_bridge: Sender<DownstreamMessages>,
        tx_outgoing: Sender<json_rpc::Message>,
        first_job_received: bool,
        extranonce2_len: usize,
        difficulty_mgmt: DownstreamDifficultyConfig,
        upstream_difficulty_config: Arc<Mutex<UpstreamDifficultyConfig>>,
    ) -> Self {
        Downstream {
            connection_id,
            authorized_names,
            extranonce1,
            version_rolling_mask,
            version_rolling_min_bit,
            tx_sv1_bridge,
            tx_outgoing,
            first_job_received,
            extranonce2_len,
            difficulty_mgmt,
            upstream_difficulty_config,
            last_call_to_update_hr: 0,
            last_notify: None,
        }
    }
}

/// Implements `IsServer` for `Downstream` to handle the SV1 messages.
impl IsServer<'static> for Downstream {
    /// Handle the incoming `mining.configure` message which is received after a Downstream role is
    /// subscribed and authorized. Contains the version rolling mask parameters.
    fn handle_configure(
        &mut self,
        request: &client_to_server::Configure,
    ) -> (Option<server_to_client::VersionRollingParams>, Option<bool>) {
        info!("Down: Handling mining.configure: {:?}", &request);
        let (version_rolling_mask, version_rolling_min_bit_count) =
            crate::shared::utils::sv1_rolling(request);

        self.version_rolling_mask = Some(version_rolling_mask.clone());
        self.version_rolling_min_bit = Some(version_rolling_min_bit_count.clone());

        (
            Some(server_to_client::VersionRollingParams::new(
                    version_rolling_mask,version_rolling_min_bit_count
            ).expect("Version mask invalid, automatic version mask selection not supported, please change it in carte::downstream_sv1::mod.rs")),
            Some(false),
        )
    }

    /// Handle the response to a `mining.subscribe` message received from the client.
    /// The subscription messages are erroneous and just used to conform the SV1 protocol spec.
    /// Because no one unsubscribed in practice, they just unplug their machine.
    fn handle_subscribe(&self, request: &client_to_server::Subscribe) -> Vec<(String, String)> {
        info!("Down: Handling mining.subscribe: {:?}", &request);

        let set_difficulty_sub = (
            "mining.set_difficulty".to_string(),
            super::new_subscription_id(),
        );
        let notify_sub = (
            "mining.notify".to_string(),
            "ae6812eb4cd7735a302a8a9dd95cf71f".to_string(),
        );

        vec![set_difficulty_sub, notify_sub]
    }

    fn handle_authorize(&self, _request: &client_to_server::Authorize) -> bool {
        if self.authorized_names.is_empty() {
            true
        } else {
            // when downstream is already authorized we do not want return an ok response otherwise
            // the sv1 proxy could thing that we are saying that downstream produced a valid share.
            warn!("Downstream is trying to authorize again, this should not happen");
            false
        }
    }

    /// When miner find the job which meets requested difficulty, it can submit share to the server.
    /// Only [Submit](client_to_server::Submit) requests for authorized user names can be submitted.
    fn handle_submit(&self, request: &client_to_server::Submit<'static>) -> bool {
        info!("Down: Handling mining.submit: {:?}", &request);

        // TODO: Check if receiving valid shares by adding diff field to Downstream

        if self.first_job_received {
            let to_send = SubmitShareWithChannelId {
                channel_id: self.connection_id,
                share: request.clone(),
                extranonce: self.extranonce1.clone(),
                extranonce2_len: self.extranonce2_len,
                version_rolling_mask: self.version_rolling_mask.clone(),
            };
            self.tx_sv1_bridge
                .try_send(DownstreamMessages::SubmitShares(to_send))
                .unwrap();
        };
        true
    }

    /// Indicates to the server that the client supports the mining.set_extranonce method.
    fn handle_extranonce_subscribe(&self) {}

    /// Checks if a Downstream role is authorized.
    fn is_authorized(&self, name: &str) -> bool {
        self.authorized_names.contains(&name.to_string())
    }

    /// Authorizes a Downstream role.
    fn authorize(&mut self, name: &str) {
        self.authorized_names.push(name.to_string());
    }

    /// Sets the `extranonce1` field sent in the SV1 `mining.notify` message to the value specified
    /// by the SV2 `OpenExtendedMiningChannelSuccess` message sent from the Upstream role.
    fn set_extranonce1(
        &mut self,
        _extranonce1: Option<Extranonce<'static>>,
    ) -> Extranonce<'static> {
        self.extranonce1.clone().try_into().unwrap()
    }

    /// Returns the `Downstream`'s `extranonce1` value.
    fn extranonce1(&self) -> Extranonce<'static> {
        self.extranonce1.clone().try_into().unwrap()
    }

    /// Sets the `extranonce2_size` field sent in the SV1 `mining.notify` message to the value
    /// specified by the SV2 `OpenExtendedMiningChannelSuccess` message sent from the Upstream role.
    fn set_extranonce2_size(&mut self, _extra_nonce2_size: Option<usize>) -> usize {
        self.extranonce2_len
    }

    /// Returns the `Downstream`'s `extranonce2_size` value.
    fn extranonce2_size(&self) -> usize {
        self.extranonce2_len
    }

    /// Returns the version rolling mask.
    fn version_rolling_mask(&self) -> Option<HexU32Be> {
        self.version_rolling_mask.clone()
    }

    /// Sets the version rolling mask.
    fn set_version_rolling_mask(&mut self, mask: Option<HexU32Be>) {
        self.version_rolling_mask = mask;
    }

    /// Sets the minimum version rolling bit.
    fn set_version_rolling_min_bit(&mut self, mask: Option<HexU32Be>) {
        self.version_rolling_min_bit = mask
    }

    fn notify(&mut self) -> Result<json_rpc::Message, sv1_api::error::Error> {
        unreachable!()
    }
}

impl IsMiningDownstream for Downstream {}

impl IsDownstream for Downstream {
    fn get_downstream_mining_data(
        &self,
    ) -> roles_logic_sv2::common_properties::CommonDownstreamData {
        todo!()
    }
}

//#[cfg(test)]
//mod tests {
//    use super::*;
//
//    #[test]
//    fn gets_difficulty_from_target() {
//        let target = vec![
//            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 255, 127,
//            0, 0, 0, 0, 0,
//        ];
//        let actual = Downstream::difficulty_from_target(target).unwrap();
//        let expect = 512.0;
//        assert_eq!(actual, expect);
//    }
//}
