//! Accounts in OCaml's runtime-config JSON layout, as `mina ledger export`
//! writes them. The importer for the same layout is [`super::Account`].

use std::io::{self, Write};

use ledger::{Account, AuthRequired, BaseLedger, FpExt, Permissions, Timing};
use mina_node_account::AccountPublicKey;
use mina_p2p_messages::v2::{
    LedgerHash, MinaBaseVerificationKeyWireStableV1, ReceiptChainHash, StateHash, TokenIdKeyHash,
};
use serde::Serialize;

/// One account of `Runtime_config.Accounts.t`. Fields are in OCaml's record
/// order; `[@default]` fields at their default are left out, as
/// `ppx_deriving_yojson` does.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/runtime_config/runtime_config.ml#L344-L359>
#[derive(Debug, Serialize, PartialEq)]
pub struct ExportedAccount {
    pub pk: String,
    pub balance: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delegate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<ExportedTiming>,
    pub token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    pub receipt_chain_hash: String,
    pub voting_for: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zkapp: Option<ExportedZkapp>,
    pub permissions: ExportedPermissions,
    pub token_symbol: String,
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/runtime_config/runtime_config.ml#L72-L80>
#[derive(Debug, Serialize, PartialEq)]
pub struct ExportedTiming {
    pub initial_minimum_balance: String,
    pub cliff_time: String,
    pub cliff_amount: String,
    pub vesting_period: String,
    pub vesting_increment: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct ExportedVerificationKeyPerm {
    pub auth: &'static str,
    pub txn_version: String,
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/runtime_config/runtime_config.ml#L229-L245>
#[derive(Debug, Serialize, PartialEq)]
pub struct ExportedPermissions {
    pub edit_state: &'static str,
    pub send: &'static str,
    pub receive: &'static str,
    pub access: &'static str,
    pub set_delegate: &'static str,
    pub set_permissions: &'static str,
    pub set_verification_key: ExportedVerificationKeyPerm,
    pub set_zkapp_uri: &'static str,
    pub edit_action_state: &'static str,
    pub set_token_symbol: &'static str,
    pub increment_nonce: &'static str,
    pub set_voting_for: &'static str,
    pub set_timing: &'static str,
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/runtime_config/runtime_config.ml#L312-L320>
#[derive(Debug, Serialize, PartialEq)]
pub struct ExportedZkapp {
    pub app_state: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_key: Option<String>,
    pub zkapp_version: String,
    pub action_state: Vec<String>,
    pub last_action_slot: u32,
    pub proved_state: bool,
    pub zkapp_uri: String,
}

/// `Currency.to_mina_string`: whole MINA, then the nanomina remainder with
/// its trailing zeros stripped.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/currency/currency.ml#L82-L95>
pub fn to_mina_string(nanomina: u64) -> String {
    const PRECISION: u64 = 1_000_000_000;
    let whole = nanomina / PRECISION;
    let remainder = nanomina % PRECISION;
    if remainder == 0 {
        whole.to_string()
    } else {
        let decimals = format!("{remainder:09}");
        format!("{whole}.{}", decimals.trim_end_matches('0'))
    }
}

/// `Auth_required.to_yojson`.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/runtime_config/runtime_config.ml#L92-L102>
fn auth(auth: AuthRequired) -> &'static str {
    match auth {
        AuthRequired::None => "none",
        AuthRequired::Either => "either",
        AuthRequired::Proof => "proof",
        AuthRequired::Signature => "signature",
        AuthRequired::Impossible => "impossible",
        // Legacy only; never in a Mesa ledger.
        AuthRequired::Both => "both",
    }
}

fn permissions(p: &Permissions<AuthRequired>) -> ExportedPermissions {
    ExportedPermissions {
        edit_state: auth(p.edit_state),
        send: auth(p.send),
        receive: auth(p.receive),
        access: auth(p.access),
        set_delegate: auth(p.set_delegate),
        set_permissions: auth(p.set_permissions),
        set_verification_key: ExportedVerificationKeyPerm {
            auth: auth(p.set_verification_key.auth),
            txn_version: p.set_verification_key.txn_version.as_u32().to_string(),
        },
        set_zkapp_uri: auth(p.set_zkapp_uri),
        edit_action_state: auth(p.edit_action_state),
        set_token_symbol: auth(p.set_token_symbol),
        increment_nonce: auth(p.increment_nonce),
        set_voting_for: auth(p.set_voting_for),
        set_timing: auth(p.set_timing),
    }
}

impl ExportedAccount {
    /// `Accounts.Single.of_account a None`.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/genesis_ledger_helper/lib/genesis_ledger_helper_lib.ml#L189-L309>
    pub fn of_account(account: &Account) -> Self {
        let timing = match &account.timing {
            Timing::Untimed => None,
            Timing::Timed {
                initial_minimum_balance,
                cliff_time,
                cliff_amount,
                vesting_period,
                vesting_increment,
            } => Some(ExportedTiming {
                initial_minimum_balance: to_mina_string(initial_minimum_balance.as_u64()),
                cliff_time: cliff_time.as_u32().to_string(),
                cliff_amount: to_mina_string(cliff_amount.as_u64()),
                vesting_period: vesting_period.as_u32().to_string(),
                vesting_increment: to_mina_string(vesting_increment.as_u64()),
            }),
        };
        let zkapp = account.zkapp.as_ref().map(|zkapp| ExportedZkapp {
            app_state: zkapp.app_state.iter().map(|f| f.to_decimal()).collect(),
            verification_key: zkapp.verification_key.as_ref().map(|vk| {
                MinaBaseVerificationKeyWireStableV1::from(vk.vk())
                    .to_base64()
                    .expect("bin_prot write to a Vec cannot fail")
            }),
            zkapp_version: zkapp.zkapp_version.to_string(),
            action_state: zkapp.action_state.iter().map(|f| f.to_decimal()).collect(),
            last_action_slot: zkapp.last_action_slot.as_u32(),
            proved_state: zkapp.proved_state,
            zkapp_uri: String::from_utf8_lossy(&zkapp.zkapp_uri).into_owned(),
        });
        let nonce = account.nonce.as_u32();
        Self {
            pk: AccountPublicKey::from(account.public_key.clone()).to_string(),
            balance: to_mina_string(account.balance.as_u64()),
            delegate: account
                .delegate
                .clone()
                .map(|pk| AccountPublicKey::from(pk).to_string()),
            timing,
            token: TokenIdKeyHash::from(account.token_id.clone()).to_string(),
            nonce: (nonce != 0).then(|| nonce.to_string()),
            receipt_chain_hash: ReceiptChainHash::from(account.receipt_chain_hash.clone())
                .to_string(),
            voting_for: StateHash::from_fp(account.voting_for.0).to_string(),
            zkapp,
            permissions: permissions(&account.permissions),
            token_symbol: String::from_utf8_lossy(account.token_symbol.as_bytes()).into_owned(),
        }
    }
}

/// Writes accounts as a JSON array, one account per line, in the order given.
/// `mina ledger hash --ledger-file` and the runtime config read it as
/// `Runtime_config.Accounts.t`.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/cli/src/init/client.ml#L752-L772>
pub fn write_accounts<'a, W: Write>(
    mut out: W,
    accounts: impl IntoIterator<Item = &'a Account>,
) -> io::Result<()> {
    out.write_all(b"[")?;
    let mut first = true;
    for account in accounts {
        out.write_all(if first { b"\n" } else { b",\n" })?;
        first = false;
        serde_json::to_writer(&mut out, &ExportedAccount::of_account(account))?;
    }
    out.write_all(b"\n]\n")?;
    Ok(())
}

/// The Merkle root of a ledger holding `accounts` at indexes 0, 1, ... in
/// order, as `mina ledger hash --ledger-file` computes it.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/cli/src/init/client.ml#L841-L888>
pub fn ledger_hash_of_accounts(
    accounts: &[super::Account],
) -> Result<LedgerHash, super::AccountConfigError> {
    let db = ledger::Database::create_with_token_owners(crate::ledger::LEDGER_DEPTH as u8);
    let mut mask = ledger::Mask::new_root(db);
    for config in accounts {
        let account = config.to_account()?;
        mask.get_or_create_account(account.id(), account)
            .map_err(|_| super::AccountConfigError::InvalidBigInt)?;
    }
    Ok(LedgerHash::from_fp(mask.merkle_root()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mina_string_matches_ocaml() {
        // Values from OCaml exports (`mina ledger export`).
        assert_eq!(to_mina_string(1_000), "0.000001");
        assert_eq!(to_mina_string(2), "0.000000002");
        assert_eq!(to_mina_string(49_460_000_000_000), "49460");
        assert_eq!(to_mina_string(49_460_001_557_310), "49460.00155731");
        assert_eq!(to_mina_string(95_486_111), "0.095486111");
        assert_eq!(to_mina_string(0), "0");
    }

    /// Accounts from OCaml 4.0.2's `mina ledger export staking-epoch-ledger`
    /// on mainnet (the Mesa genesis staking ledger), one per line: timed,
    /// zkApp with and without verification key, proved state, zkApp URI,
    /// custom tokens and symbols, non-default permissions and txn versions.
    /// The fixture is normalized to compact account lines. This checks compact
    /// serialization, not OCaml's whole-file pretty-printer output.
    #[test]
    fn export_matches_ocaml_export() {
        let sample = include_str!("testdata/ocaml-ledger-export-sample.json");
        let lines: Vec<&str> = sample
            .lines()
            .filter(|l| l.starts_with('{'))
            .map(|l| l.trim_end_matches(','))
            .collect();
        assert_eq!(lines.len(), 17);
        let mut accounts = Vec::new();
        for line in &lines {
            let config: super::super::Account = serde_json::from_str(line).unwrap();
            let account = config.to_account().unwrap();
            let exported = serde_json::to_string(&ExportedAccount::of_account(&account)).unwrap();
            assert_eq!(&exported, line);
            accounts.push(account);
        }

        let mut out = Vec::new();
        write_accounts(&mut out, &accounts).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), sample);
    }

    /// OCaml 4.0.2's `mina ledger hash --ledger-file` on the sample file.
    const OCAML_SAMPLE_HASH: &str = "jwUATMQxpP7A3xwoW7uEwS3M83zGi3prDUPSqEMpqUzvssSMaW3";

    #[test]
    fn hash_matches_ocaml_ledger_hash() {
        let sample = include_str!("testdata/ocaml-ledger-export-sample.json");
        let accounts: Vec<super::super::Account> = serde_json::from_str(sample).unwrap();
        assert_eq!(
            ledger_hash_of_accounts(&accounts).unwrap().to_string(),
            OCAML_SAMPLE_HASH
        );
    }
}
