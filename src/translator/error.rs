use std::{convert::Infallible, fmt, sync::PoisonError};

pub type ProxyResult<'a, T> = core::result::Result<T, Error<'a>>;

#[derive(Debug)]
pub enum Error<'a> {
    /// Errors due to invalid extranonce from upstream
    InvalidExtranonce(String),
    /// Errors from `roles_logic_sv2` crate.
    RolesSv2Logic(roles_logic_sv2::errors::Error),
    V1Protocol(sv1_api::error::Error<'a>),
    // Locking Errors
    PoisonLock,
    // used to handle SV2 protocol error messages from pool
    #[allow(clippy::enum_variant_names)]
    TargetError(roles_logic_sv2::errors::Error),
    Infallible(Infallible),
    ImpossibleToOpenChannnel,
    #[allow(clippy::enum_variant_names)]
    AsyncChannelError,
}

impl From<Infallible> for Error<'_> {
    fn from(v: Infallible) -> Self {
        Self::Infallible(v)
    }
}

impl<'a> fmt::Display for Error<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::InvalidExtranonce(e) => write!(f, "InvalidExtranonce {}", e),
            Error::RolesSv2Logic(e) => write!(f, "RolesSv2Logic {}", e),
            Error::V1Protocol(e) => write!(f, "V1Protocol {}", e),
            Error::PoisonLock => write!(f, "PoisonLock"),
            Error::TargetError(e) => write!(f, "TargetError {}", e),
            Error::Infallible(e) => write!(f, "Infallible {}", e),
            Error::ImpossibleToOpenChannnel => write!(f, "ImpossibleToOpenChannnel"),
            Error::AsyncChannelError => write!(f, "AsyncChannelError"),
        }
    }
}

impl<T> From<PoisonError<T>> for Error<'_> {
    fn from(_: PoisonError<T>) -> Self {
        Self::PoisonLock
    }
}
impl<T> From<tokio::sync::mpsc::error::SendError<T>> for Error<'_> {
    fn from(_value: tokio::sync::mpsc::error::SendError<T>) -> Self {
        Self::AsyncChannelError
    }
}
impl From<roles_logic_sv2::Error> for Error<'_> {
    fn from(value: roles_logic_sv2::Error) -> Self {
        Self::RolesSv2Logic(value)
    }
}
