#[cfg(target_family = "wasm")]
use gloo_utils::format::JsValueSerdeExt;
use mina_node::rpc::*;
#[cfg(target_family = "wasm")]
use wasm_bindgen::prelude::*;

use super::RpcSender;

#[derive(Clone)]
#[cfg_attr(target_family = "wasm", wasm_bindgen)]
pub struct TransitionFrontier {
    #[allow(unused)]
    sender: RpcSender,
}

#[derive(Clone)]
#[cfg_attr(target_family = "wasm", wasm_bindgen)]
pub struct TransitionFrontierBestChain {
    #[allow(unused)]
    sender: RpcSender,
}

impl TransitionFrontier {
    pub fn new(sender: RpcSender) -> Self {
        Self { sender }
    }
}

#[cfg_attr(target_family = "wasm", wasm_bindgen)]
impl TransitionFrontier {
    pub fn best_chain(&self) -> TransitionFrontierBestChain {
        TransitionFrontierBestChain {
            sender: self.sender.clone(),
        }
    }
}

impl TransitionFrontierBestChain {
    async fn _user_commands(&self) -> Option<RpcTransitionFrontierUserCommandsResponse> {
        self.sender
            .oneshot_request(RpcRequest::TransitionFrontierUserCommandsGet)
            .await
    }
}

#[cfg(not(target_family = "wasm"))]
impl TransitionFrontierBestChain {
    pub async fn user_commands(&self) -> Option<RpcTransitionFrontierUserCommandsResponse> {
        self._user_commands().await
    }
}

#[cfg(target_family = "wasm")]
#[cfg_attr(target_family = "wasm", wasm_bindgen)]
impl TransitionFrontierBestChain {
    pub async fn user_commands(&self) -> JsValue {
        JsValue::from_serde(&self._user_commands().await).unwrap_or_default()
    }
}

impl TransitionFrontierBestChain {
    async fn _headers(
        &self,
        query: RpcBestChainHeadersQuery,
    ) -> Option<RpcBestChainHeadersGetResponse> {
        self.sender
            .oneshot_request(RpcRequest::BestChainHeadersGet(query))
            .await
    }

    async fn _block(&self, query: RpcBestChainBlockQuery) -> Option<RpcBestChainBlockGetResponse> {
        self.sender
            .oneshot_request(RpcRequest::BestChainBlockGet(query))
            .await
    }
}

/// A height or a state hash, as a page's route names a block.
pub fn parse_block_id(id: &str) -> Result<RpcBestChainBlockQuery, String> {
    let id = id.trim();
    if !id.is_empty() && id.len() <= 10 && id.bytes().all(|b| b.is_ascii_digit()) {
        return id
            .parse()
            .map(RpcBestChainBlockQuery::Height)
            .map_err(|err| format!("invalid height: {err}"));
    }
    id.parse()
        .map(RpcBestChainBlockQuery::Hash)
        .map_err(|err| format!("not a height or a state hash: {err}"))
}

#[cfg(target_family = "wasm")]
#[cfg_attr(target_family = "wasm", wasm_bindgen)]
impl TransitionFrontierBestChain {
    /// MINAWEB-RPC1: the best chain's headers, newest first: at most
    /// `limit`, at or below `max_height`.
    pub async fn headers(&self, limit: Option<usize>, max_height: Option<u32>) -> JsValue {
        let res = self
            ._headers(RpcBestChainHeadersQuery { limit, max_height })
            .await;
        JsValue::from_serde(&res).unwrap_or_default()
    }

    /// MINAWEB-RPC1: one best-chain block by height or state hash; `null`
    /// if the frontier hasn't got it.
    pub async fn block(&self, id: String) -> Result<JsValue, JsValue> {
        let query = parse_block_id(&id)?;
        let res = self._block(query).await.flatten();
        Ok(JsValue::from_serde(&res).unwrap_or_default())
    }
}
