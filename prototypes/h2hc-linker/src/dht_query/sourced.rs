//! Queries for the conductor-facing protocol: the same per-peer requests as
//! the REST routes, but each answer is returned unflattened and labelled with
//! the authority that gave it. Publish-buffer data is only used when no
//! authority has the data, and is labelled as such.

use super::*;
use h2hc_protocol::{ActivityOptions, DataSource, ErrorKind, ProtocolError, QueryOptions, Sourced};
use holochain_types::prelude::{AnyDhtHash, ChainQueryFilter};
use std::future::Future;

fn internal(e: impl std::fmt::Display) -> ProtocolError {
    ProtocolError::new(ErrorKind::Internal, e.to_string())
}

fn remote_options(o: &ActivityOptions) -> GetActivityOptions {
    GetActivityOptions {
        include_valid_activity: o.include_valid_activity,
        include_rejected_activity: o.include_rejected_activity,
        include_warrants: o.include_warrants,
        include_full_records: o.include_full_records,
    }
}

/// Ask up to `count` peers in parallel. With `race`, return the first
/// non-empty answer; otherwise every answer, each labelled with its peer.
async fn fan_out<T, F, Fut>(
    peers: Vec<(AgentPubKey, Url)>,
    count: usize,
    race: bool,
    is_empty: impl Fn(&T) -> bool,
    send: F,
) -> Vec<Sourced<T>>
where
    T: Send + 'static,
    F: Fn(AgentPubKey, Url) -> Fut,
    Fut: Future<Output = LinkerResult<Option<T>>> + Send + 'static,
{
    let mut set = tokio::task::JoinSet::new();
    for (agent, url) in peers.into_iter().take(count.max(1)) {
        let request = send(agent.clone(), url);
        set.spawn(async move { (agent, request.await) });
    }
    let mut out = Vec::new();
    while let Some(joined) = set.join_next().await {
        let Ok((responder, Ok(Some(data)))) = joined else {
            continue;
        };
        let answer = Sourced {
            source: DataSource::Authority { responder },
            data,
        };
        if race && !is_empty(&answer.data) {
            set.abort_all();
            return vec![answer];
        }
        out.push(answer);
    }
    out
}

impl DhtQuery {
    fn query_timeout(&self, options: &QueryOptions) -> Duration {
        options
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(self.timeout)
    }

    async fn space_and_peers(
        &self,
        dna_hash: &DnaHash,
        loc: u32,
    ) -> Result<(DynSpace, Vec<(AgentPubKey, Url)>), ProtocolError> {
        let space = self
            .gateway_kitsune
            .get_or_create_space(dna_hash)
            .await
            .map_err(internal)?;
        let peers = self
            .get_peers_for_location(&space, loc)
            .await
            .map_err(internal)?;
        Ok((space, peers))
    }

    pub async fn sourced_get(
        &self,
        dna_hash: &DnaHash,
        hash: AnyDhtHash,
        options: &QueryOptions,
    ) -> Result<Vec<Sourced<WireOps>>, ProtocolError> {
        let loc = hash.get_loc();
        let (space, peers) = self.space_and_peers(dna_hash, loc).await?;
        let no_peers = peers.is_empty();
        let (pending, timeout) = (self.pending.clone(), self.query_timeout(options));
        let mut out = fan_out(
            peers,
            options.remote_agent_count as usize,
            options.as_race,
            is_wire_ops_empty,
            |agent, url| {
                Self::send_get_request(
                    space.clone(),
                    agent,
                    url,
                    hash.clone(),
                    pending.clone(),
                    timeout,
                )
            },
        )
        .await;
        if out.iter().all(|a| is_wire_ops_empty(&a.data)) {
            if let Some(local) = self.local_get_logged(&hash).await {
                out = vec![Sourced {
                    source: DataSource::LinkerBuffer,
                    data: local,
                }];
            }
        }
        if out.is_empty() && no_peers {
            return Err(ProtocolError::new(
                ErrorKind::NoPeers { loc },
                "no authorities known",
            ));
        }
        Ok(out)
    }

    pub async fn sourced_get_links(
        &self,
        dna_hash: &DnaHash,
        key: WireLinkKey,
        options: &QueryOptions,
    ) -> Result<Vec<Sourced<WireLinkOps>>, ProtocolError> {
        let loc = key.base.get_loc();
        let (space, peers) = self.space_and_peers(dna_hash, loc).await?;
        let no_peers = peers.is_empty();
        let (pending, timeout) = (self.pending.clone(), self.query_timeout(options));
        let is_empty = |ops: &WireLinkOps| ops.creates.is_empty() && ops.deletes.is_empty();
        let mut out = fan_out(
            peers,
            options.remote_agent_count as usize,
            options.as_race,
            is_empty,
            |agent, url| {
                Self::send_get_links_request(
                    space.clone(),
                    agent,
                    url,
                    key.clone(),
                    pending.clone(),
                    timeout,
                )
            },
        )
        .await;
        if out.iter().all(|a| is_empty(&a.data)) {
            if let Some(local) = self.local_get_links_logged(&key).await {
                out = vec![Sourced {
                    source: DataSource::LinkerBuffer,
                    data: local,
                }];
            }
        }
        if out.is_empty() && no_peers {
            return Err(ProtocolError::new(
                ErrorKind::NoPeers { loc },
                "no authorities known",
            ));
        }
        Ok(out)
    }

    pub async fn sourced_count_links(
        &self,
        dna_hash: &DnaHash,
        query: WireLinkQuery,
        options: &QueryOptions,
    ) -> Result<Vec<Sourced<CountLinksResponse>>, ProtocolError> {
        let loc = query.base.get_loc();
        let (space, peers) = self.space_and_peers(dna_hash, loc).await?;
        if peers.is_empty() {
            return Err(ProtocolError::new(
                ErrorKind::NoPeers { loc },
                "no authorities known",
            ));
        }
        let (pending, timeout) = (self.pending.clone(), self.query_timeout(options));
        Ok(fan_out(
            peers,
            options.remote_agent_count as usize,
            options.as_race,
            |_| false,
            |agent, url| {
                Self::send_count_links_request(
                    space.clone(),
                    agent,
                    url,
                    query.clone(),
                    pending.clone(),
                    timeout,
                )
            },
        )
        .await)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn sourced_agent_activity(
        &self,
        dna_hash: &DnaHash,
        agent: AgentPubKey,
        filter: ChainQueryFilter,
        activity: &ActivityOptions,
        count: usize,
        race: bool,
        timeout: Duration,
    ) -> Result<Vec<Sourced<AgentActivityResponse>>, ProtocolError> {
        let loc = agent.get_loc();
        let (space, peers) = self.space_and_peers(dna_hash, loc).await?;
        if peers.is_empty() {
            return Err(ProtocolError::new(
                ErrorKind::NoPeers { loc },
                "no authorities known",
            ));
        }
        let (pending, options) = (self.pending.clone(), remote_options(activity));
        Ok(fan_out(
            peers,
            count,
            race,
            |_| false,
            |to_agent, url| {
                Self::send_get_agent_activity_request(
                    space.clone(),
                    to_agent,
                    url,
                    agent.clone(),
                    filter.clone(),
                    options.clone(),
                    pending.clone(),
                    timeout,
                )
            },
        )
        .await)
    }

    pub async fn sourced_must_get_agent_activity(
        &self,
        dna_hash: &DnaHash,
        author: AgentPubKey,
        filter: ChainFilter,
        options: &QueryOptions,
    ) -> Result<Vec<Sourced<MustGetAgentActivityResponse>>, ProtocolError> {
        let loc = author.get_loc();
        let (space, peers) = self.space_and_peers(dna_hash, loc).await?;
        if peers.is_empty() {
            return Err(ProtocolError::new(
                ErrorKind::NoPeers { loc },
                "no authorities known",
            ));
        }
        let (pending, timeout) = (self.pending.clone(), self.query_timeout(options));
        Ok(fan_out(
            peers,
            options.remote_agent_count as usize,
            options.as_race,
            |_| false,
            |to_agent, url| {
                Self::send_must_get_agent_activity_request(
                    space.clone(),
                    to_agent,
                    url,
                    author.clone(),
                    filter.clone(),
                    pending.clone(),
                    timeout,
                )
            },
        )
        .await)
    }

    pub(crate) fn default_timeout(&self) -> Duration {
        self.timeout
    }
}
