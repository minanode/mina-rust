//! What a web node page needs for its roles (MINAWEB-P2-WRITE):
//! a sent transaction's place in the best chain, a key's
//! stake in the epoch ledgers, the consensus constants and time, and the
//! SNARK worker as the native `/snarker/control` and `/snarker/included`
//! show it. Each reads only: the roles themselves are chosen when the node
//! starts (`run`).

#[cfg(target_family = "wasm")]
use gloo_utils::format::JsValueSerdeExt;
use ledger::{AccountId, TokenId};
use mina_node::{
    account::AccountPublicKey,
    external_snark_worker::{fee_rule_problem, RpcSnarkWorkerControl},
    rpc::*,
    snark_pool::ProverAccount,
};
use mina_p2p_messages::v2;
use serde::Serialize;
#[cfg(target_family = "wasm")]
use wasm_bindgen::prelude::*;

use super::RpcSender;

#[derive(Clone)]
#[cfg_attr(target_family = "wasm", wasm_bindgen)]
pub struct Roles {
    #[allow(unused)]
    sender: RpcSender,
}

/// Nanomina as a decimal string: a JavaScript number loses precision above
/// 2^53, which a balance can exceed.
fn decimal<S: serde::Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.to_string())
}

fn decimal_opt<S: serde::Serializer>(
    value: &Option<u64>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match value {
        Some(value) => serializer.serialize_str(&value.to_string()),
        None => serializer.serialize_none(),
    }
}

/// A command found in the best chain.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct RoleInclusion {
    pub height: u32,
    pub block_hash: String,
    /// Blocks on top of it in the best chain.
    pub depth: u32,
    /// Why it failed, if it was included as failed.
    pub failure: Option<String>,
}

/// A look for a command in the newest blocks of the best chain: the tip it
/// searched from, and where the command is, if there. With the tip the page
/// can tell a block that left the best chain from one that aged out of the
/// search.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct RoleInclusionLook {
    pub tip_height: u32,
    pub depth_searched: u32,
    pub found: Option<RoleInclusion>,
}

/// A key's stake in one epoch ledger, in nanomina.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct RoleLedgerStake {
    pub hash: String,
    /// `None` when the node couldn't read the ledger (not synced yet).
    #[serde(serialize_with = "decimal_opt")]
    pub delegated_stake: Option<u64>,
    pub delegators: Option<usize>,
    #[serde(serialize_with = "decimal")]
    pub total_currency: u64,
}

/// A key's stake as the best tip's consensus state sees it: the staking
/// ledger decides this epoch's slots, the next epoch ledger the next's.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct RoleStake {
    pub epoch: u32,
    pub best_tip_height: u32,
    pub staking: RoleLedgerStake,
    pub next: RoleLedgerStake,
}

/// The worker with what OCaml's snark pool would say of its key and fee,
/// as the native `GET /snarker/control` answers.
#[derive(Serialize)]
pub struct RoleWorkerControl {
    #[serde(flatten)]
    pub control: RpcSnarkWorkerControl,
    pub account_exists: Option<bool>,
    pub fee_problem: Option<String>,
}

impl Roles {
    pub fn new(sender: RpcSender) -> Self {
        Self { sender }
    }

    async fn ledger_account(&self, key: &AccountPublicKey) -> Option<Option<ledger::Account>> {
        let accounts: Vec<ledger::Account> = self
            .sender
            .oneshot_request(RpcRequest::LedgerAccountsGet(
                AccountQuery::PubKeyWithTokenId(key.clone(), v2::TokenIdKeyHash::default()),
            ))
            .await?;
        Some(accounts.into_iter().next())
    }

    /// Where a command with this hash is in the newest `depth` blocks of
    /// the best chain; `None` if the node has no best tip or didn't answer.
    pub async fn inclusion(&self, hash: &str, depth: u32) -> Option<RoleInclusionLook> {
        let depth = depth.max(1);
        let chain: RpcBestChainResponse = self
            .sender
            .oneshot_request(RpcRequest::BestChain(depth))
            .await?;
        let tip = chain.last().map(|block| block.height())?;
        let found = chain.iter().rev().find_map(|block| {
            let command = block
                .commands_iter()
                .find(|command| command.data.hash().is_ok_and(|h| h.to_string() == hash))?;
            Some(RoleInclusion {
                height: block.height(),
                block_hash: block.hash().to_string(),
                depth: tip.saturating_sub(block.height()),
                failure: match &command.status {
                    v2::MinaBaseTransactionStatusStableV2::Applied => None,
                    v2::MinaBaseTransactionStatusStableV2::Failed(failures) => {
                        Some(format!("{failures:?}"))
                    }
                },
            })
        });
        Some(RoleInclusionLook {
            tip_height: tip,
            depth_searched: chain.len() as u32,
            found,
        })
    }

    async fn ledger_stake(
        &self,
        key: &AccountPublicKey,
        hash: &v2::LedgerHash,
        total_currency: u64,
    ) -> RoleLedgerStake {
        let account_id = mina_signer::CompressedPubKey::try_from(key.clone())
            .ok()
            .map(|key| AccountId::new(key, TokenId::default()));
        let delegators = match account_id {
            Some(account_id) => self
                .sender
                .oneshot_request::<RpcLedgerAccountDelegatorsGetResponse>(
                    RpcRequest::LedgerAccountDelegatorsGet(
                        DelegatorsQuery::Ledger(hash.clone()),
                        account_id,
                    ),
                )
                .await
                .flatten(),
            None => None,
        };
        RoleLedgerStake {
            hash: hash.to_string(),
            delegated_stake: delegators
                .as_ref()
                .map(|accounts| accounts.iter().map(|a| a.balance.as_u64()).sum()),
            delegators: delegators.as_ref().map(Vec::len),
            total_currency,
        }
    }

    /// A key's stake in the best tip's staking and next epoch ledgers.
    pub async fn stake(&self, public_key: &str) -> Result<Option<RoleStake>, String> {
        let key: AccountPublicKey = public_key
            .trim()
            .parse()
            .map_err(|_| "not a Mina public key".to_owned())?;
        let Some(chain) = self
            .sender
            .oneshot_request::<RpcBestChainResponse>(RpcRequest::BestChain(1))
            .await
        else {
            return Ok(None);
        };
        let Some(tip) = chain.last() else {
            return Ok(None);
        };
        let consensus = tip.consensus_state();
        let staking = &consensus.staking_epoch_data.ledger;
        let next = &consensus.next_epoch_data.ledger;
        Ok(Some(RoleStake {
            epoch: consensus.epoch_count.as_u32(),
            best_tip_height: tip.height(),
            staking: self
                .ledger_stake(&key, &staking.hash, staking.total_currency.as_u64())
                .await,
            next: self
                .ledger_stake(&key, &next.hash, next.total_currency.as_u64())
                .await,
        }))
    }

    pub async fn worker_control(&self) -> Option<RoleWorkerControl> {
        let control: RpcSnarkWorkerControl = self
            .sender
            .oneshot_request(RpcRequest::SnarkWorkerControlGet)
            .await?;
        let (account_exists, fee_problem) = match (&control.public_key, control.fee) {
            // Without a best tip the account query would wait for one.
            (Some(key), Some(fee)) if control.best_tip_height.is_some() => {
                match self.ledger_account(key).await {
                    Some(account) => {
                        let account = ProverAccount::from_account(account.as_ref());
                        (
                            Some(matches!(account, ProverAccount::Present { .. })),
                            fee_rule_problem(fee, account),
                        )
                    }
                    None => (None, None),
                }
            }
            _ => (None, None),
        };
        Some(RoleWorkerControl {
            control,
            account_exists,
            fee_problem,
        })
    }
}

#[cfg(target_family = "wasm")]
#[cfg_attr(target_family = "wasm", wasm_bindgen)]
impl Roles {
    /// Where a command with this hash is in the newest `depth` blocks, with
    /// the tip searched from; null when the node has no best tip.
    #[wasm_bindgen(js_name = inclusion)]
    pub async fn js_inclusion(&self, hash: String, depth: u32) -> JsValue {
        JsValue::from_serde(&self.inclusion(&hash, depth).await).unwrap_or_default()
    }

    /// A key's stake in the best tip's staking and next epoch ledgers.
    #[wasm_bindgen(js_name = stake)]
    pub async fn js_stake(&self, public_key: String) -> Result<JsValue, JsValue> {
        let stake = self.stake(&public_key).await?;
        JsValue::from_serde(&stake).map_err(|e| e.to_string().into())
    }

    pub async fn consensus_constants(&self) -> JsValue {
        let res = self
            .sender
            .oneshot_request::<RpcConsensusConstantsGetResponse>(RpcRequest::ConsensusConstantsGet)
            .await;
        JsValue::from_serde(&res).unwrap_or_default()
    }

    /// The current slot and epoch, with the slot's start and end.
    pub async fn consensus_time(&self) -> JsValue {
        let res = self
            .sender
            .oneshot_request::<RpcConsensusTimeGetResponse>(RpcRequest::ConsensusTimeGet(
                ConsensusTimeQuery::Now,
            ))
            .await
            .flatten();
        JsValue::from_serde(&res).unwrap_or_default()
    }

    /// The SNARK worker, as the native `GET /snarker/control` answers.
    pub async fn snarker_control(&self) -> JsValue {
        JsValue::from_serde(&self.worker_control().await).unwrap_or_default()
    }

    /// Our work that blocks included, as `GET /snarker/included` answers.
    pub async fn snarker_included(&self) -> JsValue {
        let res = self
            .sender
            .oneshot_request::<RpcSnarkWorkIncludedGetResponse>(RpcRequest::SnarkWorkIncludedGet)
            .await;
        JsValue::from_serde(&res).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_cross_as_exact_decimal_strings() {
        let stake = RoleLedgerStake {
            hash: "jx".to_owned(),
            delegated_stake: Some(u64::MAX),
            delegators: Some(1),
            total_currency: 9_007_199_254_740_993,
        };
        let json = serde_json::to_value(&stake).unwrap();
        assert_eq!(json["delegated_stake"], "18446744073709551615");
        assert_eq!(json["total_currency"], "9007199254740993");
        let unread = RoleLedgerStake {
            delegated_stake: None,
            ..stake
        };
        assert!(serde_json::to_value(&unread).unwrap()["delegated_stake"].is_null());
    }
}
