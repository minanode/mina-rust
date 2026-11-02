//! Account keys for the page: a new key made in the browser, and the public
//! key of one the visitor imports. The node never sees either: they are for
//! the page's own key screen, which keeps the key in memory and hands it to
//! the signer or to `run` (MINAWEB-P2-WRITE).

use gloo_utils::format::JsValueSerdeExt;
use mina_node::account::{AccountPublicKey, AccountSecretKey};
use wasm_bindgen::prelude::*;

/// A new account key, from the browser's CSPRNG (`crypto.getRandomValues`,
/// through `getrandom`): `{ publicKey, privateKey }`, both base58.
#[wasm_bindgen]
pub fn generate_account_key() -> JsValue {
    let key = AccountSecretKey::rand();
    let pair = serde_json::json!({
        "publicKey": key.public_key().to_string(),
        "privateKey": key.to_string(),
    });
    JsValue::from_serde(&pair).unwrap_or_default()
}

/// The public key (B62...) of a private key (EK...). The error never
/// repeats the value, which is a private key.
#[wasm_bindgen]
pub fn account_public_key(private_key: String) -> Result<String, JsError> {
    let key: AccountSecretKey = private_key
        .trim()
        .parse()
        .map_err(|_| JsError::new("not a Mina private key (EK..., base58)"))?;
    drop(private_key);
    Ok(key.public_key().to_string())
}

/// Whether a string is a valid public key (B62...), with its checksum.
#[wasm_bindgen]
pub fn account_public_key_valid(public_key: String) -> bool {
    public_key.trim().parse::<AccountPublicKey>().is_ok()
}
