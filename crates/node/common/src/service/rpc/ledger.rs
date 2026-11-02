#[cfg(target_family = "wasm")]
use gloo_utils::format::JsValueSerdeExt;
use mina_node::rpc::*;
#[cfg(target_family = "wasm")]
use wasm_bindgen::prelude::*;

use super::RpcSender;

#[derive(Clone)]
#[cfg_attr(target_family = "wasm", wasm_bindgen)]
pub struct Ledger {
    #[allow(unused)]
    sender: RpcSender,
}

#[derive(Clone)]
#[cfg_attr(target_family = "wasm", wasm_bindgen)]
pub struct LedgerSelected {
    #[allow(unused)]
    sender: RpcSender,
}

#[derive(Clone)]
#[cfg_attr(target_family = "wasm", wasm_bindgen)]
pub struct LedgerAccounts {
    #[allow(unused)]
    sender: RpcSender,
}

impl Ledger {
    pub fn new(sender: RpcSender) -> Self {
        Self { sender }
    }
}

#[cfg_attr(target_family = "wasm", wasm_bindgen)]
impl Ledger {
    pub fn latest(&self) -> LedgerSelected {
        LedgerSelected {
            sender: self.sender.clone(),
        }
    }
}

#[cfg_attr(target_family = "wasm", wasm_bindgen)]
impl LedgerSelected {
    pub fn accounts(&self) -> LedgerAccounts {
        LedgerAccounts {
            sender: self.sender.clone(),
        }
    }
}

impl LedgerAccounts {
    /// The accounts for a public key in the best tip's ledger, any token,
    /// summarised; empty if the ledger has none. `None` if the node gave no
    /// answer.
    pub async fn _get(&self, public_key: &str) -> Result<Option<Vec<RpcAccountSummary>>, String> {
        let key: mina_node::account::AccountPublicKey = public_key
            .trim()
            .parse()
            .map_err(|_| "not a public key".to_owned())?;
        let accounts: Option<RpcLedgerAccountsResponse> = self
            .sender
            .oneshot_request(RpcRequest::LedgerAccountsGet(
                AccountQuery::SinglePublicKey(key),
            ))
            .await;
        Ok(accounts.map(|accounts| accounts.iter().map(RpcAccountSummary::from).collect()))
    }

    async fn _all(&self) -> Option<RpcLedgerSlimAccountsResponse> {
        self.sender
            .oneshot_request(RpcRequest::LedgerAccountsGet(AccountQuery::All))
            .await
    }
}

#[cfg(not(target_family = "wasm"))]
impl LedgerAccounts {
    pub async fn all(&self) -> Option<RpcLedgerSlimAccountsResponse> {
        self._all().await
    }
}

#[cfg(target_family = "wasm")]
#[cfg_attr(target_family = "wasm", wasm_bindgen)]
impl LedgerAccounts {
    /// MINAWEB: one key's accounts at the best tip, summarised.
    pub async fn get(&self, public_key: String) -> Result<JsValue, JsValue> {
        let res = self._get(&public_key).await?;
        Ok(JsValue::from_serde(&res).unwrap_or_default())
    }

    pub async fn all(&self) -> JsValue {
        JsValue::from_serde(&self._all().await).unwrap_or_default()
    }
}
