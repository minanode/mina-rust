//! Account keys for the page: a new key made in the browser, and the public
//! key of one the visitor imports. The node never sees either: they are for
//! the page's own key screen, which keeps the key in memory and hands it to
//! the signer or to `run` (MINAWEB-P2-WRITE). A key can also come from, or
//! go to, a Mina encrypted key file (MINAWEB-KEYFILE-IMPORT).
//!
//! The private keys and passwords these take, and the private key text
//! they give, are wiped from the WASM's memory when dropped (`Zeroizing`):
//! the linear memory doesn't shrink, so freed bytes stay until reused.

use mina_node::{
    account::{AccountPublicKey, AccountSecretKey, KeyFileError},
    core::thread,
};
use wasm_bindgen::prelude::*;
use zeroize::Zeroizing;

/// The most Argon2i memory a key file may ask of this tab: twice the 128
/// MiB OCaml and `mina wallet generate` use. The derivation's memory comes
/// from the node's own linear memory, which doesn't shrink afterwards.
const KEY_FILE_MAX_MEMORY: u32 = 256 * 1024 * 1024;

/// `{ publicKey, privateKey }`, both base58. The private key's text is
/// copied into the JS string and wiped here.
fn key_pair(key: &AccountSecretKey) -> JsValue {
    let private_key = Zeroizing::new(key.to_string());
    let pair = js_sys::Object::new();
    let set =
        |name: &str, value: &str| js_sys::Reflect::set(&pair, &name.into(), &value.into()).is_ok();
    if set("publicKey", &key.public_key().to_string()) && set("privateKey", &private_key) {
        pair.into()
    } else {
        JsValue::UNDEFINED
    }
}

/// A new account key, from the browser's CSPRNG (`crypto.getRandomValues`,
/// through `getrandom`): `{ publicKey, privateKey }`, both base58.
#[wasm_bindgen]
pub fn generate_account_key() -> JsValue {
    key_pair(&AccountSecretKey::rand())
}

/// The public key (B62...) of a private key (EK...). The error never
/// repeats the value, which is a private key.
#[wasm_bindgen]
pub fn account_public_key(private_key: String) -> Result<String, JsError> {
    let private_key = Zeroizing::new(private_key);
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

/// The account key in a Mina encrypted key file (the JSON OCaml and `mina
/// wallet generate` write): `{ publicKey, privateKey }`, as
/// [`generate_account_key`]. Argon2i takes seconds, so it runs on a thread
/// of its own, never on the page's. The error is a code only
/// ([`KeyFileError`]: `wrong-password`, `not-a-key-file`, `unsupported`,
/// `not-an-account-key`, `too-expensive`), and never repeats the file or
/// the password.
#[wasm_bindgen]
pub async fn account_key_from_file(file: String, password: String) -> Result<JsValue, JsError> {
    let password = Zeroizing::new(password);
    let key = thread::spawn(move || {
        AccountSecretKey::from_key_file_text(&file, &password, KEY_FILE_MAX_MEMORY)
    })
    .join_async()
    .await
    .map_err(|_| JsError::new("failed"))?
    .map_err(|error: KeyFileError| JsError::new(&error.to_string()))?;
    Ok(key_pair(&key))
}

/// A private key (EK...) as a Mina encrypted key file's text, with
/// `password` and OCaml's default cost, for a native node to read. On a
/// thread of its own, as [`account_key_from_file`]. The error never
/// repeats the key.
#[wasm_bindgen]
pub async fn account_key_to_file(private_key: String, password: String) -> Result<String, JsError> {
    let private_key = Zeroizing::new(private_key);
    let password = Zeroizing::new(password);
    let key: AccountSecretKey = private_key
        .trim()
        .parse()
        .map_err(|_| JsError::new("not a Mina private key (EK..., base58)"))?;
    drop(private_key);
    thread::spawn(move || key.to_key_file_text(&password))
        .join_async()
        .await
        .map_err(|_| JsError::new("failed"))?
        .map_err(|_| JsError::new("failed"))
}
