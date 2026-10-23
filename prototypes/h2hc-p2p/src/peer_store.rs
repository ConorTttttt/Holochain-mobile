//! The peer store a zero-arc phone exposes: it knows no peers, because peer
//! discovery happens on the linker.

use kitsune2_api::*;
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct EmptyPeerStore;

impl PeerStore for EmptyPeerStore {
    fn insert(&self, _agent_list: Vec<Arc<AgentInfoSigned>>) -> BoxFut<'_, K2Result<()>> {
        Box::pin(async {
            Err(K2Error::other(
                "this conductor reaches the network through an H2HC linker and keeps no peer store",
            ))
        })
    }

    fn remove(&self, _agent_id: AgentId) -> BoxFut<'_, K2Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn get(&self, _agent: AgentId) -> BoxFut<'_, K2Result<Option<Arc<AgentInfoSigned>>>> {
        Box::pin(async { Ok(None) })
    }

    fn get_all(&self) -> BoxFut<'_, K2Result<Vec<Arc<AgentInfoSigned>>>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn get_by_overlapping_storage_arc(
        &self,
        _arc: DhtArc,
    ) -> BoxFut<'_, K2Result<Vec<Arc<AgentInfoSigned>>>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn get_near_location(
        &self,
        _loc: u32,
        _limit: usize,
    ) -> BoxFut<'_, K2Result<Vec<Arc<AgentInfoSigned>>>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn get_by_url(&self, _peer_url: Url) -> BoxFut<'_, K2Result<Vec<Arc<AgentInfoSigned>>>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}
