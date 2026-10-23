//! A Holochain conductor whose network is an H2HC linker.
//!
//! Needs the `ConductorBuilder::with_p2p_factory` patch
//! (`patches/holochain-0.7.1-p2p-factory.patch`).

pub use h2hc_p2p::{FrameChannel, H2hcOptions, SourcePolicy};

use holochain::conductor::config::ConductorConfig;
use holochain::conductor::error::ConductorResult;
use holochain::conductor::{ConductorBuilder, ConductorHandle};
use kitsune2_api::BoxFut;
use lair_keystore_api::types::SharedLockedArray;

/// Build a conductor that runs no Kitsune2 and reaches the network only
/// through the linker that `connect` opens a channel to.
pub async fn build_phone_conductor<C>(
    config: ConductorConfig,
    passphrase: SharedLockedArray,
    connect: C,
    options: H2hcOptions,
) -> ConductorResult<ConductorHandle>
where
    C: FnOnce() -> BoxFut<'static, Result<FrameChannel, h2hc_protocol::ProtocolError>>
        + Send
        + 'static,
{
    ConductorBuilder::new()
        .config(config)
        .passphrase(Some(passphrase))
        .no_print_setup()
        .with_p2p_factory(h2hc_p2p::factory(connect, options))
        .build()
        .await
}
