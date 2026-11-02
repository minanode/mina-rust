//! Explicit node error events for the opt-in node error reporter (R43e.2).
//!
//! A service that meets such an event hands it to the sink the reporter
//! installs; without one (no `--node-error-url`), nothing is built. The
//! events never feed back into the state machine.

use std::sync::OnceLock;

use ledger::scan_state::transaction_logic::valid;
use mina_p2p_messages::v2::MinaBaseUserCommandStableV2;

/// Each failed transaction's JSON and why it failed.
pub type TransactionFailures = Vec<(serde_json::Value, String)>;

type Sink = Box<dyn Fn(TransactionFailures) + Send + Sync>;

static TRANSACTION_FAILURE_SINK: OnceLock<Sink> = OnceLock::new();

/// Installs the sink once; later calls are ignored.
pub fn set_transaction_failure_sink(sink: impl Fn(TransactionFailures) + Send + Sync + 'static) {
    let _ = TRANSACTION_FAILURE_SINK.set(Box::new(sink));
}

/// Transactions that failed to apply while a staged ledger diff was made
/// for a block we produce: OCaml reports them as a node error.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/block_producer/block_producer.ml#L236-L250>
pub fn transaction_inclusion_failures(failures: &[(valid::UserCommand, String)]) {
    let Some(sink) = TRANSACTION_FAILURE_SINK.get() else {
        return;
    };
    if failures.is_empty() {
        return;
    }
    let failures = failures
        .iter()
        .map(|(command, error)| {
            let command = MinaBaseUserCommandStableV2::from(&command.forget_check());
            (
                serde_json::to_value(&command).unwrap_or(serde_json::Value::Null),
                error.clone(),
            )
        })
        .collect();
    sink(failures);
}
