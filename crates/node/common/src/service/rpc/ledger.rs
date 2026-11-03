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

// The light-account RPC binds the account and block metadata to one ledger
// snapshot. Separate BestChain and LedgerAccountsGet calls would race a tip
// change. Keep this adapter web-only; native callers already have /light.
#[cfg(any(target_family = "wasm", test))]
fn account_query(public_key: &str) -> Result<RpcLightAccountQuery, String> {
    use mina_node::account::AccountPublicKey;
    let key = (public_key.len() <= 64)
        .then(|| public_key.parse::<AccountPublicKey>().ok())
        .flatten()
        .and_then(|key| mina_signer::CompressedPubKey::try_from(key).ok())
        .ok_or("invalid public key")?;
    Ok(RpcLightAccountQuery {
        ledger: LightLedgerKind::Staged,
        account_ids: vec![ledger::AccountId::create(key, ledger::TokenId::default())],
        block: None,
    })
}

#[cfg(any(target_family = "wasm", test))]
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LedgerAccountResponse {
    account: FullAccount,
    block_height: u32,
    state_hash: mina_p2p_messages::v2::StateHash,
    ledger_hash: mina_p2p_messages::v2::LedgerHash,
    ledger_kind: LightLedgerKind,
}

#[cfg(any(target_family = "wasm", test))]
#[derive(serde::Serialize)]
struct FullAccount {
    /// The complete stable account encoding, including permissions and the VK.
    #[serde(flatten)]
    inner: mina_p2p_messages::v2::MinaBaseAccountBinableArgStableV2,
    /// Convenient canonical decimal fields, matching GraphQL's zkappState.
    #[serde(rename = "zkappState")]
    zkapp_state: Option<Vec<String>>,
}

#[cfg(any(target_family = "wasm", test))]
fn account_response(response: RpcLightAccountResponse) -> Result<LedgerAccountResponse, String> {
    let mut entries = response.map_err(|error| error.to_string())?;
    if entries.len() != 1 {
        return Err("expected exactly one account response".to_owned());
    }
    let entry = entries.pop().unwrap().map_err(|error| error.to_string())?;
    let zkapp_state = entry.account.zkapp.as_ref().map(|zkapp| {
        zkapp
            .app_state
            .iter()
            .map(|field| field.to_decimal())
            .collect()
    });
    Ok(LedgerAccountResponse {
        account: FullAccount {
            inner: entry.account,
            zkapp_state,
        },
        block_height: entry.block.height,
        state_hash: entry.block.state_hash,
        ledger_hash: entry.ledger_hash,
        ledger_kind: entry.ledger,
    })
}

#[cfg(target_family = "wasm")]
#[wasm_bindgen]
impl LedgerSelected {
    /// Read the default-token account from the best tip's staged ledger.
    /// Returns the full stable account plus account.zkappState (decimal field
    /// strings or null), blockHeight, stateHash, ledgerHash and ledgerKind.
    /// Rejects while bootstrapping, for a missing account or on RPC failure.
    pub async fn account(&self, public_key: String) -> Result<JsValue, JsValue> {
        let query = account_query(&public_key).map_err(|error| JsValue::from_str(&error))?;
        let response = self
            .sender
            .oneshot_request::<RpcLightAccountResponse>(RpcRequest::LightAccount(query))
            .await
            .ok_or_else(|| JsValue::from_str("node RPC closed"))?;
        let response = account_response(response).map_err(|error| JsValue::from_str(&error))?;
        JsValue::from_serde(&response).map_err(|error| JsValue::from_str(&error.to_string()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use mina_p2p_messages::{
        bigint::BigInt,
        v2::{LedgerHash, StateHash},
    };

    // From an OCaml node's devnet account query, 2026-10-27, block 580555.
    const KEY: &str = "B62qjJ9sukEjgDcZtgjiPzkEUdxawoEtFcdArmZDiJCFePE4VrcCHT2";
    const ROOT: &str =
        "6244084871035803089170261294273093095529370576804460419223898627927100319520";
    const STATE: &str = "3NLaWhHksivjGYwHZH33NTMfR5d7aKFxtN9hNfYJuServniex2Rd";
    const LEDGER: &str = "jwSwN8J4Rby7zFbkuK8hFR3VxYoRhUt9LcPPcVPsh7tsHnzHWCs";

    #[test]
    fn web_account_preserves_snapshot_and_full_account() {
        let query = account_query(KEY).unwrap();
        assert_eq!(query.ledger, LightLedgerKind::Staged);
        assert_eq!(query.block, None);
        assert_eq!(query.account_ids.len(), 1);
        assert_eq!(query.account_ids[0].token_id, ledger::TokenId::default());
        assert!(account_query("not a public key").is_err());
        assert!(account_query(&"a".repeat(1000)).is_err());

        let mut account = ledger::Account::empty();
        account.public_key = query.account_ids[0].public_key.clone();
        let mut zkapp = ledger::ZkAppAccount::default();
        zkapp.app_state[2] = ROOT.parse().unwrap();
        account.zkapp = Some(Box::new(zkapp));
        let stable = mina_p2p_messages::v2::MinaBaseAccountBinableArgStableV2::from(&account);
        let entry = RpcLightAccount {
            ledger: LightLedgerKind::Staged,
            ledger_hash: LEDGER.parse::<LedgerHash>().unwrap(),
            block: LightBlockRef {
                state_hash: STATE.parse::<StateHash>().unwrap(),
                height: 580555,
                global_slot_since_genesis: 0,
            },
            index: 0,
            leaf_hash: BigInt::zero(),
            account: stable.clone(),
            merkle_path: vec![],
        };
        let json =
            serde_json::to_value(account_response(Ok(vec![Ok(Box::new(entry))])).unwrap()).unwrap();
        assert_eq!(json["account"]["zkappState"][2], ROOT);
        assert_eq!(json["account"]["zkappState"].as_array().unwrap().len(), 32);
        assert_eq!(json["stateHash"], STATE);
        assert_eq!(json["blockHeight"], 580555);
        assert_eq!(json["ledgerHash"], LEDGER);
        assert_eq!(json["ledgerKind"], "staged");
        let decoded: mina_p2p_messages::v2::MinaBaseAccountBinableArgStableV2 =
            serde_json::from_value(json["account"].clone()).unwrap();
        assert_eq!(decoded, stable);
    }

    #[test]
    fn web_account_refuses_unavailable_reads() {
        assert!(account_response(Err(RpcLightError::NotSynced)).is_err());
        assert!(account_response(Ok(vec![Err(RpcLightError::AccountNotFound(
            LightLedgerKind::Staged
        ))]))
        .is_err());
        assert!(account_response(Ok(vec![])).is_err());
    }
}
