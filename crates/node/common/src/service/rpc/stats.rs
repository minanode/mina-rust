#[cfg(target_family = "wasm")]
use gloo_utils::format::JsValueSerdeExt;
#[cfg(target_family = "wasm")]
use mina_node::rpc::*;
#[cfg(target_family = "wasm")]
use wasm_bindgen::prelude::*;

use super::RpcSender;

#[derive(Clone)]
#[cfg_attr(target_family = "wasm", wasm_bindgen)]
pub struct Stats {
    #[allow(unused)]
    sender: RpcSender,
}

impl Stats {
    pub fn new(sender: RpcSender) -> Self {
        Self { sender }
    }
}

#[cfg(target_family = "wasm")]
#[cfg_attr(target_family = "wasm", wasm_bindgen)]
impl Stats {
    pub async fn actions(&self, id: JsValue) -> Result<JsValue, JsValue> {
        let query = if id.is_falsy() {
            ActionStatsQuery::SinceStart
        } else if id.as_string().is_some_and(|s| s == "latest") {
            ActionStatsQuery::ForLatestBlock
        } else {
            let id = id.into_serde().map_err(|err| err.to_string())?;
            ActionStatsQuery::ForBlockWithId(id)
        };

        let res = self
            .sender
            .oneshot_request::<RpcActionStatsGetResponse>(RpcRequest::ActionStatsGet(query))
            .await
            .flatten();
        Ok(JsValue::from_serde(&res).unwrap_or_default())
    }

    pub async fn sync(&self, limit: Option<usize>) -> JsValue {
        let query = SyncStatsQuery { limit };
        let res = self
            .sender
            .oneshot_request::<RpcSyncStatsGetResponse>(RpcRequest::SyncStatsGet(query))
            .await
            .flatten();
        JsValue::from_serde(&res).unwrap_or_default()
    }

    /// Receipts newest first; or, given `hashes` (an array of state hashes,
    /// at most 128), those blocks' receipts. See `VerificationQuery`.
    pub async fn verification(
        &self,
        limit: Option<usize>,
        offset: Option<usize>,
        hashes: JsValue,
    ) -> Result<JsValue, JsValue> {
        let hashes = if hashes.is_falsy() {
            Vec::new()
        } else {
            let hashes: Vec<String> = hashes.into_serde().map_err(|err| err.to_string())?;
            hashes
                .iter()
                .take(mina_node::verification::MAX_RECEIPT_PAGE_SIZE)
                .map(|hash| hash.parse())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| format!("invalid state hash: {err}"))?
        };
        let query = mina_node::verification::VerificationQuery {
            limit,
            offset: offset.unwrap_or(0),
            hashes,
        };
        let res = self
            .sender
            .oneshot_request::<RpcVerificationStatsGetResponse>(RpcRequest::VerificationStatsGet(
                query,
            ))
            .await;
        Ok(JsValue::from_serde(&res).unwrap_or_default())
    }

    pub async fn block_producer(&self) -> JsValue {
        let res = self
            .sender
            .oneshot_request::<RpcBlockProducerStatsGetResponse>(RpcRequest::BlockProducerStatsGet)
            .await
            .flatten();
        JsValue::from_serde(&res).unwrap_or_default()
    }
}
