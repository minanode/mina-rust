#[cfg(target_family = "wasm")]
use gloo_utils::format::JsValueSerdeExt;
use mina_node::rpc::*;
use mina_p2p_messages::v2;
#[cfg(target_family = "wasm")]
use wasm_bindgen::prelude::*;

use super::RpcSender;

pub mod signing;

#[derive(Clone)]
#[cfg_attr(target_family = "wasm", wasm_bindgen)]
pub struct TransactionPool {
    #[allow(unused)]
    sender: RpcSender,
}

#[derive(Clone)]
#[cfg_attr(target_family = "wasm", wasm_bindgen)]
pub struct TransactionPoolInject {
    #[allow(unused)]
    sender: RpcSender,
}

impl TransactionPool {
    pub fn new(sender: RpcSender) -> Self {
        Self { sender }
    }

    async fn _get(&self) -> Option<RpcTransactionPoolResponse> {
        self.sender
            .oneshot_request(RpcRequest::TransactionPoolGet)
            .await
    }
}

#[cfg_attr(target_family = "wasm", wasm_bindgen)]
impl TransactionPool {
    pub fn inject(&self) -> TransactionPoolInject {
        TransactionPoolInject {
            sender: self.sender.clone(),
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl TransactionPool {
    pub async fn get(&self) -> Option<RpcTransactionPoolResponse> {
        self._get().await
    }
}

#[cfg(target_family = "wasm")]
#[cfg_attr(target_family = "wasm", wasm_bindgen)]
impl TransactionPool {
    pub async fn get(&self) -> JsValue {
        JsValue::from_serde(&self._get().await).unwrap_or_default()
    }
}

impl TransactionPoolInject {
    async fn _zkapp(
        &self,
        command: v2::MinaBaseZkappCommandTStableV1WireStableV1,
    ) -> Option<RpcTransactionInjectResponse> {
        self.sender
            .oneshot_request(RpcRequest::TransactionInject(vec![
                v2::MinaBaseUserCommandStableV2::ZkappCommand(command),
            ]))
            .await
    }

    async fn _payment(
        &self,
        payments: Vec<RpcInjectPayment>,
    ) -> Result<Option<RpcTransactionInjectResponse>, String> {
        let res = self
            .sender
            .oneshot_request(RpcRequest::TransactionInject(
                payments
                    .into_iter()
                    .map(v2::MinaBaseUserCommandStableV2::try_from)
                    .collect::<Result<_, _>>()
                    .map_err(|err| err.to_string())?,
            ))
            .await;
        Ok(res)
    }
}

#[cfg(not(target_family = "wasm"))]
impl TransactionPoolInject {
    pub async fn zkapp(
        &self,
        command: v2::MinaBaseZkappCommandTStableV1WireStableV1,
    ) -> Option<RpcTransactionInjectResponse> {
        self._zkapp(command).await
    }

    pub async fn payment(
        &self,
        payments: Vec<RpcInjectPayment>,
    ) -> Result<Option<RpcTransactionInjectResponse>, String> {
        self._payment(payments).await
    }
}

#[cfg(target_family = "wasm")]
#[cfg_attr(target_family = "wasm", wasm_bindgen)]
impl TransactionPoolInject {
    /// Inject a signed wire-format zkApp into this node's own pool. Normal
    /// pool validation (including authorizations) runs before acceptance.
    pub async fn zkapp(&self, command: JsValue) -> Result<JsValue, JsValue> {
        let command = command.into_serde().map_err(|e| e.to_string())?;
        JsValue::from_serde(&self._zkapp(command).await).map_err(|e| e.to_string().into())
    }

    /// Sign in Rust/WASM with the node's network and inject into its pool.
    /// Amount and fee are decimal nanomina strings. Never store the key on
    /// the page: JS/CDP strings and signer allocations cannot be reliably erased.
    pub async fn sign_and_inject_payment(
        &self,
        private_key: String,
        payment: JsValue,
    ) -> Result<JsValue, JsValue> {
        let command = sign(private_key, payment, signing::payment).await?;
        self.inject_signed(command).await
    }

    /// Build P4.15's signature-only two-update transfer, sign in Rust/WASM,
    /// and inject. The recipient must exist and allow unsigned receives.
    pub async fn sign_and_inject_zkapp_transfer(
        &self,
        private_key: String,
        transfer: JsValue,
    ) -> Result<JsValue, JsValue> {
        let command = sign(private_key, transfer, signing::zkapp_transfer).await?;
        self.inject_signed(command).await
    }

    pub async fn payment(&self, payments: JsValue) -> Result<JsValue, JsValue> {
        let payments: Vec<RpcInjectPayment> = if payments.is_array() {
            payments.into_serde().map_err(|err| err.to_string())?
        } else {
            let payment = payments.into_serde().map_err(|err| err.to_string())?;
            vec![payment]
        };

        self._payment(payments)
            .await
            .map(|res| JsValue::from_serde(&res).unwrap_or_default())
            .map_err(Into::into)
    }
}

#[cfg(target_family = "wasm")]
async fn sign(
    private_key: String,
    transfer: JsValue,
    build: fn(
        &mina_node::account::AccountSecretKey,
        &signing::Transfer,
    ) -> Result<v2::MinaBaseUserCommandStableV2, String>,
) -> Result<v2::MinaBaseUserCommandStableV2, JsValue> {
    let transfer = transfer.into_serde().map_err(|e| e.to_string())?;
    let key = private_key.parse().map_err(|_| "invalid private key")?;
    drop(private_key);
    // Call-forest hashing uses Rayon. Calling it on the browser's main thread
    // would block in Atomics.wait, which browsers forbid. Transfer only Rust
    // values to the existing WASM worker pool; never serialize the key into a
    // node RPC/action or a JS worker message. Drop is not memory erasure.
    let (tx, rx) = mina_core::channels::oneshot::channel();
    rayon::spawn_fifo(move || {
        let _ = tx.send(build(&key, &transfer));
    });
    rx.await
        .map_err(|_| "signing worker stopped")?
        .map_err(Into::into)
}

#[cfg(target_family = "wasm")]
impl TransactionPoolInject {
    async fn inject_signed(
        &self,
        command: v2::MinaBaseUserCommandStableV2,
    ) -> Result<JsValue, JsValue> {
        let hash = command.hash().map_err(|e| e.to_string())?;
        let network = mina_core::NetworkConfig::global().name;
        mina_core::info!(mina_core::log::system_time();
            kind = "TransactionSignedInWasm", hash = display(&hash), network = network);
        let response: RpcTransactionInjectResponse = self
            .sender
            .oneshot_request(RpcRequest::TransactionInject(vec![command]))
            .await
            .ok_or("node shut down before injection response")?;
        let accepted = matches!(&response, RpcTransactionInjectResponse::Success(commands) if commands.len() == 1);
        JsValue::from_serde(&serde_json::json!({
            "hash": hash, "network": network, "signing": "rust-wasm",
            "accepted": accepted, "response": response,
        }))
        .map_err(|e| e.to_string().into())
    }
}
