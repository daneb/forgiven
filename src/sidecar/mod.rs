mod protocol;
#[cfg(unix)]
mod server;
pub use protocol::NexusEvent;
#[cfg(unix)]
pub use server::SidecarServer;
