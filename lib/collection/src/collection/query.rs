use std::mem;
use std::sync::Arc;

use futures::{future, TryFutureExt};
use itertools::{Either, Itertools};
use segment::common::reciprocal_rank_fusion::rrf_scoring;
use segment::types::{Order, ScoredPoint};
use segment::utils::scored_point_ties::ScoredPointTies;
use tokio::time::Instant;

use super::Collection;
use crate::common::fetch_vectors::resolve_referenced_vectors_batch;
use crate::operations::consistency_params::ReadConsistency;
use crate::operations::shard_selector_internal::ShardSelectorInternal;
use crate::operations::types::{CollectionError, CollectionResult};
use crate::operations::universal_query::collection_query::CollectionQueryRequest;
use crate::operations::universal_query::shard_query::{
    Fusion, ScoringQuery, ShardQueryRequest, ShardQueryResponse,
};

struct IntermediateQueryInfo<'a> {
    scoring_query: Option<&'a ScoringQuery>,
    take: usize,
}

impl Collection {
    /// Returns a vector of shard responses for the given query.
    async fn query_shards_concurrently(
        &self,
        request: Arc<ShardQueryRequest>,
        read_consistency: Option<ReadConsistency>,
        shard_selection: &ShardSelectorInternal,
    ) -> CollectionResult<Vec<ShardQueryResponse>> {
        // query all shards concurrently
        let shard_holder = self.shards_holder.read().await;
        let target_shards = shard_holder.select_shards(shard_selection)?;
        let all_searches = target_shards.iter().map(|(shard, shard_key)| {
            let shard_key = shard_key.cloned();
            shard
                .query(
                    Arc::clone(&request),
                    read_consistency,
                    shard_selection.is_shard_id(),
                )
                .and_then(move |mut records| async move {
                    if shard_key.is_none() {
                        return Ok(records);
                    }
                    for batch in &mut records {
                        for point in batch {
                            point.shard_key.clone_from(&shard_key);
                        }
                    }
                    Ok(records)
                })
        });
        future::try_join_all(all_searches).await
    }

    /// To be called on the user-responding instance. Resolves ids into vectors, and merges the results from local and remote shards.
    ///
    /// This function is used to query the collection. It will return a list of scored points.
    pub async fn query(
        &self,
        request: CollectionQueryRequest,
        read_consistency: Option<ReadConsistency>,
        shard_selection: &ShardSelectorInternal,
    ) -> CollectionResult<Vec<ScoredPoint>> {
        let instant = Instant::now();

        // Turn ids into vectors, if necessary
        let ids_to_vectors = resolve_referenced_vectors_batch(
            &[(&request, shard_selection.clone())],
            self,
            |_| async { unimplemented!("lookup_from is not implemented yet") },
            read_consistency,
        )
        .await?;

        let request = Arc::new(request.try_into_shard_request(&ids_to_vectors)?);

        let all_shards_results = self
            .query_shards_concurrently(request.clone(), read_consistency, shard_selection)
            .await?;

        let mut merged_intemediates = self
            .merge_intermediate_results_from_shards(request.as_ref(), all_shards_results)
            .await?;

        let result = if let Some(ScoringQuery::Fusion(fusion)) = &request.query {
            // If the root query is a Fusion, the returned results correspond to each the prefetches.
            match fusion {
                Fusion::Rrf => rrf_scoring(merged_intemediates, request.limit, request.offset),
            }
        } else {
            // Otherwise, it will be a list with a single list of scored points.
            debug_assert_eq!(merged_intemediates.len(), 1);
            merged_intemediates
                .pop()
                .ok_or_else(|| {
                    CollectionError::service_error(
                        "Query response was expected to have one list of results.",
                    )
                })?
                .into_iter()
                .skip(request.offset)
                .take(request.limit)
                .collect()
        };

        let filter_refs = request.filter_refs();
        self.post_process_if_slow_request(instant.elapsed(), filter_refs);

        Ok(result)
    }

    /// To be called on the remote instance. Only used for the internal service.
    ///
    /// If the root query is a Fusion, the returned results correspond to each the prefetches.
    /// Otherwise, it will be a list with a single list of scored points.
    pub async fn query_internal(
        &self,
        request: ShardQueryRequest,
        read_consistency: Option<ReadConsistency>,
        shard_selection: &ShardSelectorInternal,
    ) -> CollectionResult<ShardQueryResponse> {
        let request = Arc::new(request);

        let all_shards_results = self
            .query_shards_concurrently(Arc::clone(&request), read_consistency, shard_selection)
            .await?;

        let merged = self
            .merge_intermediate_results_from_shards(request.as_ref(), all_shards_results)
            .await?;

        Ok(merged)
    }

    /// Merges the results in each shard for each intermediate query.
    /// ```text
    /// [ [shard1_result1, shard1_result2],
    ///          ↓               ↓
    ///   [shard2_result1, shard2_result2] ]
    ///
    /// = [merged_result1, merged_result2]
    /// ```
    async fn merge_intermediate_results_from_shards(
        &self,
        request: &ShardQueryRequest,
        mut all_shards_results: Vec<ShardQueryResponse>,
    ) -> CollectionResult<ShardQueryResponse> {
        let queries_for_results = intermediate_query_infos(request);
        let results_len = queries_for_results.len();
        let mut results = Vec::with_capacity(results_len);
        debug_assert!(all_shards_results
            .iter()
            .all(|shard_results| shard_results.len() == results_len));

        let collection_params = self.collection_config.read().await.params.clone();
        for (idx, intermediate_info) in queries_for_results.into_iter().enumerate() {
            let same_result_per_shard = all_shards_results
                .iter_mut()
                .map(|intermediates| mem::take(&mut intermediates[idx]));

            let order = ScoringQuery::order(intermediate_info.scoring_query, &collection_params)?;

            let intermediate_result = match order {
                Order::LargeBetter => Either::Left(
                    same_result_per_shard.kmerge_by(|a, b| ScoredPointTies(a) > ScoredPointTies(b)),
                ),
                Order::SmallBetter => Either::Right(
                    same_result_per_shard.kmerge_by(|a, b| ScoredPointTies(a) < ScoredPointTies(b)),
                ),
            }
            .dedup()
            .take(intermediate_info.take)
            .collect();

            results.push(intermediate_result);
        }

        Ok(results)
    }
}

/// Returns a list of the query that corresponds to each of the results in each shard.
///
/// Example: `[info1, info2, info3]` corresponds to `[result1, result2, result3]` of each shard
fn intermediate_query_infos(request: &ShardQueryRequest) -> Vec<IntermediateQueryInfo<'_>> {
    let has_intermediate_results = request
        .query
        .as_ref()
        .map(|sq| sq.needs_intermediate_results())
        .unwrap_or(false);

    if has_intermediate_results {
        // In case of RRF, expect the propagated intermediate results
        request
            .prefetches
            .iter()
            .map(|prefetch| IntermediateQueryInfo {
                scoring_query: prefetch.query.as_ref(),
                take: prefetch.limit,
            })
            .collect_vec()
    } else {
        // Otherwise, we expect the root result
        vec![IntermediateQueryInfo {
            scoring_query: request.query.as_ref(),
            take: request.offset + request.limit,
        }]
    }
}
