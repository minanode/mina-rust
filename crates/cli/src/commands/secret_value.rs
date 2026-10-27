//! Secret inputs must never enter clap's value-validation error context.
use std::{ffi::OsStr, marker::PhantomData, str::FromStr};

use clap::{builder::TypedValueParser, error::ErrorKind};
use mina_node::{account::AccountSecretKey, p2p::identity::SecretKey};

#[derive(Clone)]
struct SecretValueParser<T> {
    message: &'static str,
    value: PhantomData<T>,
}

impl<T: Clone + Send + Sync + FromStr + 'static> TypedValueParser for SecretValueParser<T> {
    type Value = T;

    fn parse_ref(
        &self,
        _cmd: &clap::Command,
        _arg: Option<&clap::Arg>,
        value: &OsStr,
    ) -> Result<T, clap::Error> {
        // A function returning Result<T, &str> is insufficient: clap embeds
        // the original input before displaying that error. Return a raw error
        // with no input, underlying error or usage context, including for
        // non-UTF-8 passwords. Password whitespace is significant. clap prints
        // a raw message as it is, so it ends with its own newline.
        value
            .to_str()
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| {
                clap::Error::raw(ErrorKind::ValueValidation, format!("{}\n", self.message))
            })
    }
}

fn parser<T>(message: &'static str) -> SecretValueParser<T> {
    SecretValueParser {
        message,
        value: PhantomData,
    }
}

/// A key file's password, from the environment only: on the command line it
/// would be visible to other processes for as long as this one runs. An
/// unset variable is the empty password, as the old arguments' default was.
/// A value that is not valid UTF-8 is refused without echoing it.
pub(super) fn password_from_env(name: &str) -> anyhow::Result<String> {
    Ok(optional_password_from_env(name)?.unwrap_or_default())
}

/// As `password_from_env`, keeping an unset variable apart from an empty one.
pub(super) fn optional_password_from_env(name: &str) -> anyhow::Result<Option<String>> {
    password_from_value(std::env::var_os(name))
}

/// An environment variable's value as a password, refusing one that is not
/// valid UTF-8 without echoing it.
fn password_from_value(value: Option<std::ffi::OsString>) -> anyhow::Result<Option<String>> {
    value
        .map(|value| {
            value
                .into_string()
                .map_err(|_| anyhow::anyhow!("invalid password"))
        })
        .transpose()
}

/// The positional a password used to be passed as now refuses it, without
/// echoing what was typed, and says where the password is read from instead.
pub(super) fn refused_privkey_password() -> impl TypedValueParser<Value = String> {
    refused(
        "the password is read from the MINA_PRIVKEY_PASS environment variable, not the command line",
    )
}

/// As `refused_privkey_password`, for `mina node`. Its first positional was
/// the libp2p keyfile's password, but an operator passing one value most
/// likely meant the producer key's, so the message names both variables.
pub(super) fn refused_node_password() -> impl TypedValueParser<Value = String> {
    refused(
        "the passwords are read from the environment, not the command line: \
         MINA_PRIVKEY_PASS for the block producer key, MINA_LIBP2P_PASS for the libp2p keyfile",
    )
}

/// A parser that never accepts a value: whatever is typed is answered with
/// the fixed `message`, never the value itself.
fn refused(message: &'static str) -> impl TypedValueParser<Value = String> {
    #[derive(Clone)]
    struct Refused(&'static str);
    impl TypedValueParser for Refused {
        type Value = String;
        fn parse_ref(
            &self,
            _cmd: &clap::Command,
            _arg: Option<&clap::Arg>,
            _value: &std::ffi::OsStr,
        ) -> Result<String, clap::Error> {
            Err(clap::Error::raw(
                clap::error::ErrorKind::ValueValidation,
                format!("{}\n", self.0),
            ))
        }
    }
    Refused(message)
}

pub(super) fn account_key() -> impl TypedValueParser<Value = AccountSecretKey> {
    // Preserve OCaml's strict Base58Check parsing: it does not strip whitespace.
    // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/crypto/signature_lib/private_key.ml#L48-L50>
    // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/base58_check/base58_check.ml#L62-L67>
    parser::<AccountSecretKey>("invalid secret key")
}

/// An account secret key from the environment variable `name`, parsed as
/// `account_key` parses it. An invalid value is refused without echoing it.
pub(super) fn account_key_from_env(name: &str) -> anyhow::Result<Option<AccountSecretKey>> {
    std::env::var_os(name)
        .map(|value| {
            value
                .to_str()
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| anyhow::anyhow!("{name}: invalid secret key"))
        })
        .transpose()
}

pub(super) fn p2p_key() -> impl TypedValueParser<Value = SecretKey> {
    parser::<SecretKey>("invalid secret key")
}

#[cfg(test)]
mod tests {
    use super::password_from_value;

    // Unset stays apart from empty, and a set value is used exactly,
    // whitespace included. Tested on values, not this process's environment.
    #[test]
    fn passwords_are_taken_from_environment_values() {
        assert_eq!(password_from_value(None).unwrap(), None);
        assert_eq!(
            password_from_value(Some("".into())).unwrap(),
            Some(String::new())
        );
        assert_eq!(
            password_from_value(Some(" dummy password\t".into())).unwrap(),
            Some(" dummy password\t".to_owned())
        );
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let value = std::ffi::OsString::from_vec(b"dummy-password\xff".to_vec());
            let error = password_from_value(Some(value)).unwrap_err().to_string();
            assert_eq!(error, "invalid password");
        }
    }
}
