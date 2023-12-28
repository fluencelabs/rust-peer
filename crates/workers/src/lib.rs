#![feature(try_blocks)]

pub type DealId = String;

pub type WorkerId = PeerId;

mod error;
mod key_storage;
mod persistence;
mod scope;
mod workers;

pub use error::KeyManagerError;
pub use error::WorkersError;
use fluence_libp2p::PeerId;
pub use key_storage::KeyStorage;
pub use scope::Scopes;
pub use workers::Workers;
