use roles_logic_sv2::parsers::PoolMessages;

pub mod diff_management;
#[allow(clippy::module_inception)]
pub mod upstream;
pub use upstream::Upstream;
mod task_manager;

pub type Message = PoolMessages<'static>;

#[derive(Clone, Copy, Debug)]
pub struct Sv2MiningConnection {
    _version: u16,
    _setup_connection_flags: u32,
    #[allow(dead_code)]
    setup_connection_success_flags: u32,
}
