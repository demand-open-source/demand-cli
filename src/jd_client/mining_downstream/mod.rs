mod task_manager;
use crate::shared::utils::AbortOnDrop;
use tokio::time::{timeout, Duration};

use super::{job_declarator::JobDeclarator, mining_upstream::Upstream as UpstreamMiningNode};
use roles_logic_sv2::{
    channel_logic::channel_factory::{OnNewShare, PoolChannelFactory, Share},
    common_properties::{CommonDownstreamData, IsDownstream, IsMiningDownstream},
    errors::Error,
    handlers::mining::{ParseDownstreamMiningMessages, SendTo, SupportedChannelTypes},
    job_creator::JobsCreators,
    mining_sv2::*,
    parsers::{Mining, MiningDeviceMessages},
    template_distribution_sv2::{NewTemplate, SubmitSolution},
    utils::Mutex,
};
use task_manager::TaskManager;
use tokio::{
    sync::mpsc::{Receiver as TReceiver, Sender as TSender},
    task,
};
use tracing::{debug, error, warn};

use codec_sv2::{StandardEitherFrame, StandardSv2Frame};

use bitcoin::{consensus::Decodable, TxOut};

pub type Message = MiningDeviceMessages<'static>;
pub type StdFrame = StandardSv2Frame<Message>;
pub type EitherFrame = StandardEitherFrame<Message>;

/// 1 to 1 connection with a downstream node that implement the mining (sub)protocol can be either
/// a mining device or a downstream proxy.
/// A downstream can only be linked with an upstream at a time. Support multi upstrems for
/// downstream do no make much sense.
#[derive(Debug)]
pub struct DownstreamMiningNode {
    sender: TSender<Mining<'static>>,
    pub status: DownstreamMiningNodeStatus,
    pub prev_job_id: Option<u32>,
    solution_sender: TSender<SubmitSolution<'static>>,
    withhold: bool,
    miner_coinbase_output: Vec<TxOut>,
    // used to retreive the job id of the share that we send upstream
    last_template_id: u64,
    jd: Option<Arc<Mutex<JobDeclarator>>>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum DownstreamMiningNodeStatus {
    Paired(Arc<Mutex<UpstreamMiningNode>>),
    ChannelOpened((PoolChannelFactory, Arc<Mutex<UpstreamMiningNode>>)),
    SoloMinerPaired(),
    SoloMinerChannelOpend(PoolChannelFactory),
}

impl DownstreamMiningNodeStatus {
    fn set_channel(&mut self, channel: PoolChannelFactory) -> bool {
        match self {
            DownstreamMiningNodeStatus::Paired(up) => {
                let self_ = Self::ChannelOpened((channel, up.clone()));
                let _ = std::mem::replace(self, self_);
                true
            }
            DownstreamMiningNodeStatus::ChannelOpened(_) => false,
            DownstreamMiningNodeStatus::SoloMinerPaired() => {
                let self_ = Self::SoloMinerChannelOpend(channel);
                let _ = std::mem::replace(self, self_);
                true
            }
            DownstreamMiningNodeStatus::SoloMinerChannelOpend(_) => false,
        }
    }

    pub fn get_channel(&mut self) -> &mut PoolChannelFactory {
        match self {
            DownstreamMiningNodeStatus::Paired(_) => panic!(),
            DownstreamMiningNodeStatus::ChannelOpened((channel, _)) => channel,
            DownstreamMiningNodeStatus::SoloMinerPaired() => panic!(),
            DownstreamMiningNodeStatus::SoloMinerChannelOpend(channel) => channel,
        }
    }
    fn have_channel(&self) -> bool {
        match self {
            DownstreamMiningNodeStatus::Paired(_) => false,
            DownstreamMiningNodeStatus::ChannelOpened(_) => true,
            DownstreamMiningNodeStatus::SoloMinerPaired() => false,
            DownstreamMiningNodeStatus::SoloMinerChannelOpend(_) => true,
        }
    }
    fn get_upstream(&mut self) -> Option<Arc<Mutex<UpstreamMiningNode>>> {
        match self {
            DownstreamMiningNodeStatus::Paired(up) => Some(up.clone()),
            DownstreamMiningNodeStatus::ChannelOpened((_, up)) => Some(up.clone()),
            DownstreamMiningNodeStatus::SoloMinerPaired() => None,
            DownstreamMiningNodeStatus::SoloMinerChannelOpend(_) => None,
        }
    }
    fn is_solo_miner(&mut self) -> bool {
        matches!(
            self,
            DownstreamMiningNodeStatus::SoloMinerPaired()
                | DownstreamMiningNodeStatus::SoloMinerChannelOpend(_)
        )
    }
}

use core::convert::TryInto;
use std::sync::Arc;

impl DownstreamMiningNode {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sender: TSender<Mining<'static>>,
        upstream: Option<Arc<Mutex<UpstreamMiningNode>>>,
        solution_sender: TSender<SubmitSolution<'static>>,
        withhold: bool,
        miner_coinbase_output: Vec<TxOut>,
        jd: Option<Arc<Mutex<JobDeclarator>>>,
    ) -> Self {
        let status = match upstream {
            Some(up) => DownstreamMiningNodeStatus::Paired(up),
            None => DownstreamMiningNodeStatus::SoloMinerPaired(),
        };
        Self {
            sender,
            status,
            prev_job_id: None,
            solution_sender,
            withhold,
            miner_coinbase_output,
            // set it to an arbitrary value cause when we use it we always updated it.
            // Is used before sending the share to upstream in the main loop when we have a share.
            // Is upated in the message handler that si called earlier in the main loop.
            last_template_id: 0,
            jd,
        }
    }

    /// Strat listen for downstream mining node. Return as soon as one downstream connect.
    pub async fn start(
        self_mutex: Arc<Mutex<Self>>,
        mut receiver: TReceiver<Mining<'static>>,
    ) -> AbortOnDrop {
        let task_manager = TaskManager::initialize();
        let abortable = task_manager
            .safe_lock(|t| t.get_aborter())
            .unwrap()
            .unwrap();
        let factory_abortable = DownstreamMiningNode::set_channel_factory(self_mutex.clone());
        TaskManager::add_set_channel_factory(task_manager.clone(), factory_abortable)
            .await
            .unwrap();
        let main_task = task::spawn(async move {
            while let Some(message) = receiver.recv().await {
                DownstreamMiningNode::next(&self_mutex, message).await;
            }
        });
        TaskManager::add_main_task(task_manager, main_task.into())
            .await
            .unwrap();
        abortable
    }

    // When we do pooled minig we create a channel factory when the pool send a open extended
    // mining channel success
    fn set_channel_factory(self_mutex: Arc<Mutex<Self>>) -> AbortOnDrop {
        tokio::task::spawn(async move {
            if !self_mutex.safe_lock(|s| s.status.is_solo_miner()).unwrap() {
                // Safe unwrap already checked if it contains an upstream withe `is_solo_miner`
                let upstream = self_mutex
                    .safe_lock(|s| s.status.get_upstream().unwrap())
                    .unwrap();
                let factory = UpstreamMiningNode::take_channel_factory(upstream).await;
                self_mutex
                    .safe_lock(|s| {
                        s.status.set_channel(factory);
                    })
                    .unwrap();
            }
        })
        .into()
    }

    /// Parse the received message and relay it to the right upstream
    pub async fn next(self_mutex: &Arc<Mutex<Self>>, incoming: Mining<'static>) {
        let routing_logic = roles_logic_sv2::routing_logic::MiningRoutingLogic::None;

        let next_message_to_send =
            ParseDownstreamMiningMessages::handle_message_mining_deserialized(
                self_mutex.clone(),
                Ok(incoming.clone()),
                routing_logic,
            );
        Self::match_send_to(self_mutex.clone(), next_message_to_send, Some(incoming)).await;
    }

    #[async_recursion::async_recursion]
    async fn match_send_to(
        self_mutex: Arc<Mutex<Self>>,
        next_message_to_send: Result<SendTo<UpstreamMiningNode>, Error>,
        incoming: Option<Mining<'static>>,
    ) {
        match next_message_to_send {
            Ok(SendTo::RelaySameMessageToRemote(upstream_mutex)) => {
                let incoming = incoming.expect("JDC dowstream try to releay an inexistent message");
                UpstreamMiningNode::send(&upstream_mutex, incoming)
                    .await
                    .unwrap();
            }
            Ok(SendTo::RelayNewMessage(Mining::SubmitSharesExtended(mut share))) => {
                // If we have a realy new message it means that we are in a pooled mining mods.
                let upstream_mutex = self_mutex
                    .safe_lock(|s| s.status.get_upstream().unwrap())
                    .unwrap();
                // When re receive SetupConnectionSuccess we link the last_template_id with the
                // pool's job_id. The below return as soon as we have a pairable job id for the
                // template_id associated with this share.
                let last_template_id = self_mutex.safe_lock(|s| s.last_template_id).unwrap();
                let job_id_future =
                    UpstreamMiningNode::get_job_id(&upstream_mutex, last_template_id);
                let job_id = match timeout(Duration::from_secs(10), job_id_future).await {
                    Ok(job_id) => job_id,
                    Err(_) => {
                        return;
                    }
                };
                share.job_id = job_id;
                debug!(
                    "Sending valid block solution upstream, with job_id {}",
                    job_id
                );
                let message = Mining::SubmitSharesExtended(share);
                UpstreamMiningNode::send(&upstream_mutex, message)
                    .await
                    .unwrap();
            }
            Ok(SendTo::RelayNewMessage(message)) => {
                let upstream_mutex = self_mutex.safe_lock(|s| s.status.get_upstream().expect("We should return RelayNewMessage only if we are not in solo mining mode")).unwrap();
                UpstreamMiningNode::send(&upstream_mutex, message)
                    .await
                    .unwrap();
            }
            Ok(SendTo::Multiple(messages)) => {
                for message in messages {
                    Self::match_send_to(self_mutex.clone(), Ok(message), None).await;
                }
            }
            Ok(SendTo::Respond(message)) => {
                Self::send(&self_mutex, message).await.unwrap();
            }
            Ok(SendTo::None(None)) => (),
            Ok(m) => unreachable!("Unexpected message type: {:?}", m),
            Err(_) => todo!(),
        }
    }

    /// Send a message downstream
    pub async fn send(self_mutex: &Arc<Mutex<Self>>, message: Mining<'static>) -> Result<(), ()> {
        let sender = self_mutex.safe_lock(|self_| self_.sender.clone()).unwrap();
        match sender.send(message).await {
            Ok(_) => Ok(()),
            Err(_) => {
                todo!()
            }
        }
    }

    pub async fn on_new_template(
        self_mutex: &Arc<Mutex<Self>>,
        mut new_template: NewTemplate<'static>,
        pool_output: &[u8],
    ) -> Result<(), Error> {
        if !self_mutex.safe_lock(|s| s.status.have_channel()).unwrap() {
            super::IS_NEW_TEMPLATE_HANDLED.store(true, std::sync::atomic::Ordering::Release);
            return Ok(());
        }
        let mut pool_out = &pool_output[0..];
        let pool_output =
            TxOut::consensus_decode(&mut pool_out).expect("Upstream sent an invalid coinbase");
        let to_send = self_mutex
            .safe_lock(|s| {
                let channel = s.status.get_channel();
                channel.update_pool_outputs(vec![pool_output]);
                // TODO TODO TODO if this fail just reinitialize the proxy
                channel.on_new_template(&mut new_template)
            })
            .expect("Poison lock")?;
        // to_send is HashMap<channel_id, messages_to_send> but here we have only one downstream so
        // only one channel opened downstream. That means that we can take all the messages in the
        // map and send them downstream.
        let to_send = to_send.into_values();
        for message in to_send {
            let message = if let Mining::NewExtendedMiningJob(job) = message {
                let jd = self_mutex.safe_lock(|s| s.jd.clone()).unwrap().unwrap();
                jd.safe_lock(|jd| jd.coinbase_tx_prefix = job.coinbase_tx_prefix.clone())
                    .unwrap();
                jd.safe_lock(|jd| jd.coinbase_tx_suffix = job.coinbase_tx_suffix.clone())
                    .unwrap();

                Mining::NewExtendedMiningJob(job)
            } else {
                message
            };
            Self::send(self_mutex, message).await.unwrap();
        }
        // See coment on the definition of the global for memory
        // ordering
        super::IS_NEW_TEMPLATE_HANDLED.store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    pub async fn on_set_new_prev_hash(
        self_mutex: &Arc<Mutex<Self>>,
        new_prev_hash: roles_logic_sv2::template_distribution_sv2::SetNewPrevHash<'static>,
    ) -> Result<(), Error> {
        if !self_mutex.safe_lock(|s| s.status.have_channel()).unwrap() {
            return Ok(());
        }
        let job_id = self_mutex
            .safe_lock(|s| {
                let channel = s.status.get_channel();
                channel.on_new_prev_hash_from_tp(&new_prev_hash)
            })
            .unwrap()?;
        let channel_ids = self_mutex
            .safe_lock(|s| s.status.get_channel().get_extended_channels_ids())
            .unwrap();
        let channel_id = match channel_ids.len() {
            1 => channel_ids[0],
            _ => unreachable!(),
        };
        let to_send = SetNewPrevHash {
            channel_id,
            job_id,
            prev_hash: new_prev_hash.prev_hash,
            min_ntime: new_prev_hash.header_timestamp,
            nbits: new_prev_hash.n_bits,
        };
        let message = Mining::SetNewPrevHash(to_send);
        Self::send(self_mutex, message).await.unwrap();
        Ok(())
    }
}

use roles_logic_sv2::selectors::NullDownstreamMiningSelector;
impl IsDownstream for DownstreamMiningNode {
    fn get_downstream_mining_data(&self) -> CommonDownstreamData {
        CommonDownstreamData {
            header_only: false,
            work_selection: true,
            version_rolling: true,
        }
    }
}
/// It impl UpstreamMining cause the proxy act as an upstream node for the DownstreamMiningNode
impl
    ParseDownstreamMiningMessages<
        UpstreamMiningNode,
        NullDownstreamMiningSelector,
        roles_logic_sv2::routing_logic::NoRouting,
    > for DownstreamMiningNode
{
    fn get_channel_type(&self) -> SupportedChannelTypes {
        SupportedChannelTypes::Extended
    }

    fn is_work_selection_enabled(&self) -> bool {
        false
    }

    fn handle_open_standard_mining_channel(
        &mut self,
        _: OpenStandardMiningChannel,
        _: Option<Arc<Mutex<UpstreamMiningNode>>>,
    ) -> Result<SendTo<UpstreamMiningNode>, Error> {
        warn!("Ignoring OpenStandardMiningChannel");
        Ok(SendTo::None(None))
    }

    fn handle_open_extended_mining_channel(
        &mut self,
        m: OpenExtendedMiningChannel,
    ) -> Result<SendTo<UpstreamMiningNode>, Error> {
        if !self.status.is_solo_miner() {
            // Safe unwrap alreay checked if it cointains upstream with is_solo_miner
            Ok(SendTo::RelaySameMessageToRemote(
                self.status.get_upstream().unwrap(),
            ))
        } else {
            // The channel factory is created here so that we are sure that if we have a channel
            // open we have a factory and if we have a factory we have a channel open. This allowto
            // not change the semantic of Status beween solo and pooled modes
            let extranonce_len = 32;
            let range_0 = std::ops::Range { start: 0, end: 0 };
            let range_1 = std::ops::Range { start: 0, end: 16 };
            let range_2 = std::ops::Range {
                start: 16,
                end: extranonce_len,
            };
            let ids = Arc::new(Mutex::new(roles_logic_sv2::utils::GroupId::new()));
            let coinbase_outputs = self.miner_coinbase_output.clone();
            let extranonces = ExtendedExtranonce::new(range_0, range_1, range_2);
            let creator = JobsCreators::new(extranonce_len as u8);
            let share_per_min = 1.0;
            let kind = roles_logic_sv2::channel_logic::channel_factory::ExtendedChannelKind::Pool;
            let channel_factory = PoolChannelFactory::new(
                ids,
                extranonces,
                creator,
                share_per_min,
                kind,
                coinbase_outputs,
                "SOLO".to_string(),
            );
            self.status.set_channel(channel_factory);

            let request_id = m.request_id;
            let hash_rate = m.nominal_hash_rate;
            let min_extranonce_size = m.min_extranonce_size;
            let messages_res = self.status.get_channel().new_extended_channel(
                request_id,
                hash_rate,
                min_extranonce_size,
            );
            match messages_res {
                Ok(messages) => {
                    let messages = messages.into_iter().map(SendTo::Respond).collect();
                    Ok(SendTo::Multiple(messages))
                }
                Err(_) => Err(roles_logic_sv2::Error::ChannelIsNeitherExtendedNeitherInAPool),
            }
        }
    }

    fn handle_update_channel(
        &mut self,
        _: UpdateChannel,
    ) -> Result<SendTo<UpstreamMiningNode>, Error> {
        if !self.status.is_solo_miner() {
            // Safe unwrap alreay checked if it cointains upstream with is_solo_miner
            Ok(SendTo::RelaySameMessageToRemote(
                self.status.get_upstream().unwrap(),
            ))
        } else {
            todo!()
        }
    }

    fn handle_submit_shares_standard(
        &mut self,
        _: SubmitSharesStandard,
    ) -> Result<SendTo<UpstreamMiningNode>, Error> {
        warn!("Ignoring SubmitSharesStandard");
        Ok(SendTo::None(None))
    }

    fn handle_submit_shares_extended(
        &mut self,
        m: SubmitSharesExtended,
    ) -> Result<SendTo<UpstreamMiningNode>, Error> {
        match self
            .status
            .get_channel()
            .on_submit_shares_extended(m.clone())
            .unwrap()
        {
            OnNewShare::SendErrorDownstream(s) => {
                error!("Share do not meet downstream target");
                Ok(SendTo::Respond(Mining::SubmitSharesError(s)))
            }
            OnNewShare::SendSubmitShareUpstream((m, Some(template_id))) => {
                if !self.status.is_solo_miner() {
                    match m {
                        Share::Extended(share) => {
                            let for_upstream = Mining::SubmitSharesExtended(share);
                            self.last_template_id = template_id;
                            Ok(SendTo::RelayNewMessage(for_upstream))
                        }
                        // We are in an extended channel shares are extended
                        Share::Standard(_) => unreachable!(),
                    }
                } else {
                    Ok(SendTo::None(None))
                }
            }
            OnNewShare::RelaySubmitShareUpstream => unreachable!(),
            OnNewShare::ShareMeetBitcoinTarget((
                share,
                Some(template_id),
                coinbase,
                extranonce,
            )) => {
                match share {
                    Share::Extended(share) => {
                        let solution_sender = self.solution_sender.clone();
                        let solution = SubmitSolution {
                            template_id,
                            version: share.version,
                            header_timestamp: share.ntime,
                            header_nonce: share.nonce,
                            coinbase_tx: coinbase.try_into()?,
                        };
                        // The below channel should never be full is ok to block
                        solution_sender.blocking_send(solution).unwrap();
                        if !self.status.is_solo_miner() {
                            {
                                let jd = self.jd.clone();
                                let mut share = share.clone();
                                share.extranonce = extranonce.try_into().unwrap();
                                // This do not need to be put in a task manager it always return
                                // fastly
                                tokio::task::spawn(async move {
                                    JobDeclarator::on_solution(&jd.unwrap(), share).await
                                });
                            }
                        }

                        // Safe unwrap alreay checked if it cointains upstream with is_solo_miner
                        if !self.withhold && !self.status.is_solo_miner() {
                            self.last_template_id = template_id;
                            let for_upstream = Mining::SubmitSharesExtended(share);
                            Ok(SendTo::RelayNewMessage(for_upstream))
                        } else {
                            Ok(SendTo::None(None))
                        }
                    }
                    // We are in an extended channel shares are extended
                    Share::Standard(_) => unreachable!(),
                }
            }
            // When we have a ShareMeetBitcoinTarget it means that the proxy know the bitcoin
            // target that means that the proxy must have JD capabilities that means that the
            // second tuple elements can not be None but must be Some(template_id)
            OnNewShare::ShareMeetBitcoinTarget(_) => unreachable!(),
            OnNewShare::SendSubmitShareUpstream(_) => unreachable!(),
            OnNewShare::ShareMeetDownstreamTarget => Ok(SendTo::None(None)),
        }
    }

    fn handle_set_custom_mining_job(
        &mut self,
        _: SetCustomMiningJob,
    ) -> Result<SendTo<UpstreamMiningNode>, Error> {
        warn!("Ignoring SetCustomMiningJob");
        Ok(SendTo::None(None))
    }
}
impl IsMiningDownstream for DownstreamMiningNode {}
