use api::rest::schema::ShardKeySelector;
use segment::data_types::vectors::DEFAULT_VECTOR_NAME;
use segment::types::PointIdType;

use crate::operations::types::{DiscoverRequestInternal, RecommendRequestInternal, UsingVector};
use crate::operations::universal_query::collection_query::{
    self, CollectionPrefetch, CollectionQueryRequest, VectorInput, VectorQuery,
};

const EMPTY_SHARD_KEY_SELECTOR: Option<ShardKeySelector> = None;

pub trait RetrieveRequest {
    fn get_lookup_collection(&self) -> Option<&String>;

    fn get_referenced_point_ids(&self) -> Vec<PointIdType>;

    fn get_lookup_vector_name(&self) -> String;

    fn get_lookup_shard_key(&self) -> &Option<ShardKeySelector>;
}

impl RetrieveRequest for RecommendRequestInternal {
    fn get_lookup_collection(&self) -> Option<&String> {
        self.lookup_from.as_ref().map(|x| &x.collection)
    }

    fn get_referenced_point_ids(&self) -> Vec<PointIdType> {
        self.positive
            .iter()
            .chain(self.negative.iter())
            .filter_map(|example| example.as_point_id())
            .collect()
    }

    fn get_lookup_vector_name(&self) -> String {
        match &self.lookup_from {
            None => match &self.using {
                None => DEFAULT_VECTOR_NAME.to_owned(),
                Some(UsingVector::Name(vector_name)) => vector_name.clone(),
            },
            Some(lookup_from) => match &lookup_from.vector {
                None => DEFAULT_VECTOR_NAME.to_owned(),
                Some(vector_name) => vector_name.clone(),
            },
        }
    }

    fn get_lookup_shard_key(&self) -> &Option<ShardKeySelector> {
        self.lookup_from
            .as_ref()
            .map(|x| &x.shard_key)
            .unwrap_or(&EMPTY_SHARD_KEY_SELECTOR)
    }
}

impl RetrieveRequest for DiscoverRequestInternal {
    fn get_lookup_collection(&self) -> Option<&String> {
        self.lookup_from.as_ref().map(|x| &x.collection)
    }

    fn get_referenced_point_ids(&self) -> Vec<PointIdType> {
        let mut res = Vec::new();

        match &self.target {
            None => {}
            Some(example) => {
                if let Some(point_id) = example.as_point_id() {
                    res.push(point_id);
                }
            }
        }

        if let Some(context) = &self.context {
            for pair in context {
                if let Some(pos_id) = pair.positive.as_point_id() {
                    res.push(pos_id);
                }
                if let Some(neg_id) = pair.negative.as_point_id() {
                    res.push(neg_id);
                }
            }
        }

        res
    }

    fn get_lookup_vector_name(&self) -> String {
        match &self.lookup_from {
            None => match &self.using {
                None => DEFAULT_VECTOR_NAME.to_owned(),
                Some(UsingVector::Name(vector_name)) => vector_name.clone(),
            },
            Some(lookup_from) => match &lookup_from.vector {
                None => DEFAULT_VECTOR_NAME.to_owned(),
                Some(vector_name) => vector_name.clone(),
            },
        }
    }

    fn get_lookup_shard_key(&self) -> &Option<ShardKeySelector> {
        self.lookup_from
            .as_ref()
            .map(|x| &x.shard_key)
            .unwrap_or(&EMPTY_SHARD_KEY_SELECTOR)
    }
}

impl RetrieveRequest for &CollectionQueryRequest {
    fn get_lookup_collection(&self) -> Option<&String> {
        None // TODO(universal-query): Change this when we add lookup_from to CollectionQueryRequest
    }

    fn get_referenced_point_ids(&self) -> Vec<PointIdType> {
        let mut refs = Vec::new();

        if let Some(collection_query::Query::Vector(vector_query)) = &self.query {
            refs.extend(vector_query.get_referenced_ids())
        };

        for prefetch in &self.prefetch {
            refs.extend(prefetch.get_referenced_ids())
        }

        refs
    }

    fn get_lookup_vector_name(&self) -> String {
        self.using.clone() //TODO(universal-query): Update this when we add lookup_from to CollectionQueryRequest
    }

    fn get_lookup_shard_key(&self) -> &Option<ShardKeySelector> {
        &None // TODO(universal-query): Change this when we add lookup_from to CollectionQueryRequest
    }
}
impl VectorQuery<VectorInput> {
    pub fn get_referenced_ids(&self) -> Vec<&PointIdType> {
        self.flat_iter().filter_map(VectorInput::as_id).collect()
    }
}

impl CollectionPrefetch {
    fn get_referenced_ids(&self) -> Vec<PointIdType> {
        let mut refs = Vec::new();

        if let Some(collection_query::Query::Vector(vector_query)) = &self.query {
            refs.extend(vector_query.get_referenced_ids())
        };

        for prefetch in &self.prefetch {
            refs.extend(prefetch.get_referenced_ids())
        }

        refs
    }
}
