//! Edge cases of transaction application, against OCaml 4.0.4 (c9e2eac):
//! accounts created twice, zkApp commands failing at different account
//! updates, verification key changes within one command, and `valid_while`
//! bounds. Expected values follow OCaml's transaction logic, cited per test.
//!
//! Run with: cargo test -p mina-tree --test test_transaction_logic_conformance

use ark_ff::Zero;
use mina_core::constants::constraint_constants;
use mina_curves::pasta::Fp;
use mina_signer::{CompressedPubKey, Signature};
use mina_tree::{
    scan_state::{
        currency::{Amount, Balance, Fee, Length, Magnitude, Nonce, Sgn, Signed, Slot},
        transaction_logic::{
            protocol_state::{EpochData, EpochLedger, ProtocolStateView},
            signed_command::{
                Body as SignedBody, Common, PaymentPayload, SignedCommand, SignedCommandPayload,
            },
            transaction_applied::TransactionApplied,
            transaction_partially_applied::apply_transactions,
            zkapp_command::{
                verifiable, AccountPreconditions, AccountUpdate, Actions, AuthorizationKind, Body,
                CallForest, ClosedInterval, Control, Events, FeePayer, FeePayerBody, MayUseToken,
                OrIgnore, Preconditions, SetOrKeep, Update, ZkAppCommand, ZkAppPreconditions,
            },
            Memo, Transaction, TransactionFailure, TransactionStatus, UserCommand,
        },
    },
    sparse_ledger::SparseLedger,
    Account, AccountId, AuthRequired, BaseLedger, ControlTag, GetOrCreated, Mask, Permissions,
    ProofVerified, SetVerificationKey, TokenId, VerificationKey, VerificationKeyWire, ZkAppAccount,
    TXN_VERSION_CURRENT, ZKAPP_STATE_SIZE,
};

const MINA: u64 = 1_000_000_000;
const FEE: u64 = 10_000_000;

fn creation_fee() -> u64 {
    constraint_constants().account_creation_fee
}

fn key() -> CompressedPubKey {
    Account::rand().public_key
}

fn id(pk: &CompressedPubKey) -> AccountId {
    AccountId::new(pk.clone(), TokenId::default())
}

fn ledger(accounts: &[(&CompressedPubKey, u64)]) -> Mask {
    let mut ledger = Mask::new_unattached(constraint_constants().ledger_depth as usize);
    for (pk, balance) in accounts {
        let account = Account::create_with(id(pk), Balance::from_u64(*balance));
        ledger.get_or_create_account(id(pk), account).unwrap();
    }
    ledger
}

fn account(ledger: &Mask, pk: &CompressedPubKey) -> Option<Box<Account>> {
    let location = ledger.location_of_account(&id(pk))?;
    ledger.get(location)
}

fn set_account(ledger: &mut Mask, account: Account) {
    let location = ledger.location_of_account(&account.id()).unwrap();
    ledger.set(location, Box::new(account));
}

fn epoch_data() -> EpochData<Fp> {
    EpochData {
        ledger: EpochLedger {
            hash: Fp::zero(),
            total_currency: Amount::zero(),
        },
        seed: Fp::zero(),
        start_checkpoint: Fp::zero(),
        lock_checkpoint: Fp::zero(),
        epoch_length: Length::from_u32(0),
    }
}

fn view(slot: u32) -> ProtocolStateView {
    ProtocolStateView {
        snarked_ledger_hash: Fp::zero(),
        blockchain_length: Length::from_u32(0),
        min_window_density: Length::from_u32(0),
        total_currency: Amount::zero(),
        global_slot_since_genesis: Slot::from_u32(slot),
        staking_epoch_data: epoch_data(),
        next_epoch_data: epoch_data(),
    }
}

fn apply(ledger: &mut Mask, slot: u32, txns: &[Transaction]) -> Vec<TransactionApplied> {
    apply_transactions(
        constraint_constants(),
        Slot::from_u32(slot),
        &view(slot),
        ledger,
        txns,
    )
    .unwrap()
}

fn status(applied: &TransactionApplied) -> &TransactionStatus {
    applied.transaction_status()
}

fn payment(from: &CompressedPubKey, to: &CompressedPubKey, amount: u64, nonce: u32) -> Transaction {
    let payload = SignedCommandPayload {
        common: Common {
            fee: Fee::from_u64(FEE),
            fee_payer_pk: from.clone(),
            nonce: Nonce::from_u32(nonce),
            valid_until: Slot::max(),
            memo: Memo::empty(),
        },
        body: SignedBody::Payment(PaymentPayload {
            receiver_pk: to.clone(),
            amount: Amount::from_u64(amount),
        }),
    };
    Transaction::Command(UserCommand::SignedCommand(Box::new(SignedCommand {
        payload,
        signer: from.clone(),
        signature: Signature::dummy(),
    })))
}

/// An account update of `pk` that changes nothing and needs no
/// authorization.
fn body(pk: &CompressedPubKey) -> Body {
    Body {
        public_key: pk.clone(),
        token_id: TokenId::default(),
        update: Update::noop(),
        balance_change: Signed::zero(),
        increment_nonce: false,
        events: Events::empty(),
        actions: Actions::empty(),
        call_data: Fp::zero(),
        preconditions: Preconditions {
            network: ZkAppPreconditions::accept(),
            account: AccountPreconditions(
                mina_tree::scan_state::transaction_logic::zkapp_command::Account::accept(),
            ),
            valid_while: OrIgnore::Ignore,
        },
        use_full_commitment: false,
        implicit_account_creation_fee: false,
        may_use_token: MayUseToken::No,
        authorization_kind: AuthorizationKind::NoneGiven,
    }
}

fn none_given(body: Body) -> AccountUpdate {
    AccountUpdate {
        body,
        authorization: Control::NoneGiven,
    }
}

/// Signed over the full commitment, as a signed account update that isn't
/// the fee payer must be (or bump and fix its nonce) to pass the replay
/// check. Transaction application doesn't check signatures.
fn signed(mut body: Body) -> AccountUpdate {
    body.authorization_kind = AuthorizationKind::Signature;
    body.use_full_commitment = true;
    AccountUpdate {
        body,
        authorization: Control::Signature(Signature::dummy()),
    }
}

/// Proved with the key of hash `vk_hash`. Transaction application doesn't
/// check proofs: it takes them as verified and compares the key hash with
/// the account's.
fn proved(mut body: Body, vk_hash: Fp) -> AccountUpdate {
    body.authorization_kind = AuthorizationKind::Proof(vk_hash);
    AccountUpdate {
        body,
        authorization: Control::dummy_of_tag(ControlTag::Proof),
    }
}

fn amount(sgn: Sgn, amount: u64) -> Signed<Amount> {
    Signed {
        magnitude: Amount::from_u64(amount),
        sgn,
    }
}

fn command(fee_payer: &CompressedPubKey, nonce: u32, updates: Vec<AccountUpdate>) -> ZkAppCommand {
    let account_updates = updates
        .into_iter()
        .rev()
        .fold(CallForest::empty(), |forest, update| {
            forest.cons(None, update)
        });
    ZkAppCommand {
        fee_payer: FeePayer {
            body: FeePayerBody {
                public_key: fee_payer.clone(),
                fee: Fee::from_u64(FEE),
                valid_until: None,
                nonce: Nonce::from_u32(nonce),
            },
            authorization: Signature::dummy(),
        },
        account_updates,
        memo: Memo::empty(),
    }
}

fn zkapp(command: ZkAppCommand) -> Transaction {
    Transaction::Command(UserCommand::ZkAppCommand(Box::new(command)))
}

fn failures(applied: &TransactionApplied) -> Vec<Vec<TransactionFailure>> {
    match status(applied) {
        TransactionStatus::Applied => vec![],
        TransactionStatus::Failed(failures) => failures.clone(),
    }
}

/// The failure table OCaml gives a command whose only failure is `failure`
/// at account update `index` (1 for the first after the fee payer) of `n`:
/// every other account update but the fee payer is `Cancelled`.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/transaction_logic/mina_transaction_logic.ml#L1801-L1822>
fn only_failure(
    n: usize,
    index: usize,
    failure: TransactionFailure,
) -> Vec<Vec<TransactionFailure>> {
    let mut table = vec![vec![TransactionFailure::Cancelled]; n + 1];
    table[0] = vec![];
    table[index] = vec![failure];
    table
}

// Accounts created twice.

/// A second payment to an account the first created doesn't create it
/// again: one creation fee, from the first payment, as OCaml's
/// `compute_updates` charges it only to an `Added` receiver.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/transaction_logic/mina_transaction_logic.ml#L415-L425>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/transaction_logic/mina_transaction_logic.ml#L660-L690>
#[test]
fn payments_create_a_new_account_once() {
    let (alice, carol) = (key(), key());
    let mut ledger = ledger(&[(&alice, 10 * MINA)]);
    let applied = apply(
        &mut ledger,
        10,
        &[
            payment(&alice, &carol, 2 * MINA, 0),
            payment(&alice, &carol, 3 * MINA, 1),
        ],
    );
    assert!(applied.iter().all(|a| status(a).is_applied()));
    assert_eq!(ledger.num_accounts(), 2);
    let carol = account(&ledger, &carol).unwrap();
    assert_eq!(carol.balance, Balance::from_u64(5 * MINA - creation_fee()));
    let alice = account(&ledger, &alice).unwrap();
    assert_eq!(
        alice.balance,
        Balance::from_u64(10 * MINA - 5 * MINA - 2 * FEE)
    );
    assert_eq!(alice.nonce, Nonce::from_u32(2));
}

/// A payment to an existing account creates nothing and pays no fee.
#[test]
fn a_payment_to_an_existing_account_creates_nothing() {
    let (alice, bob) = (key(), key());
    let mut ledger = ledger(&[(&alice, 10 * MINA), (&bob, MINA)]);
    let applied = apply(&mut ledger, 10, &[payment(&alice, &bob, 2 * MINA, 0)]);
    assert!(status(&applied[0]).is_applied());
    assert_eq!(ledger.num_accounts(), 2);
    assert_eq!(
        account(&ledger, &bob).unwrap().balance,
        Balance::from_u64(3 * MINA)
    );
}

/// Two account updates crediting the same new account in one command:
/// only the first finds it new and pays the creation fee, as OCaml's
/// `check_account` reads the command's ledger so far. The fee comes out of
/// the credit; the command's excess counts the credit in full.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/transaction_logic/zkapp_command_logic.ml#L1285-L1290>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/transaction_logic/zkapp_command_logic.ml#L1419-L1457>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/transaction_logic/zkapp_command_logic.ml#L1821-L1830>
#[test]
fn account_updates_create_a_new_account_once() {
    let (alice, carol) = (key(), key());
    let mut ledger = ledger(&[(&alice, 10 * MINA)]);
    let credit = |amount_: u64| {
        let mut body = body(&carol);
        body.balance_change = amount(Sgn::Pos, amount_);
        body.implicit_account_creation_fee = true;
        none_given(body)
    };
    let mut debit = body(&alice);
    debit.balance_change = amount(Sgn::Neg, 3 * MINA);
    let command = command(
        &alice,
        0,
        vec![signed(debit), credit(2 * MINA), credit(MINA)],
    );
    let applied = apply(&mut ledger, 10, &[zkapp(command)]);
    assert_eq!(failures(&applied[0]), Vec::<Vec<_>>::new());
    assert_eq!(ledger.num_accounts(), 2);
    let carol = account(&ledger, &carol).unwrap();
    assert_eq!(carol.balance, Balance::from_u64(3 * MINA - creation_fee()));
}

/// Creating an account that exists is an error, as OCaml's
/// `create_new_account` returns one, and changes nothing.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/mina_ledger/ledger.ml#L482-L494>
#[test]
fn creating_an_existing_account_is_an_error() {
    use mina_tree::sparse_ledger::LedgerIntf;

    let alice = key();
    let mut ledger = ledger(&[(&alice, 10 * MINA)]);
    let root = BaseLedger::merkle_root(&mut ledger);
    let other = Account::create_with(id(&alice), Balance::from_u64(MINA));
    assert!(LedgerIntf::create_new_account(&mut ledger, id(&alice), other.clone()).is_err());
    assert!(matches!(
        ledger.get_or_create_account(id(&alice), other),
        Ok(GetOrCreated::Existed(_))
    ));
    assert_eq!(BaseLedger::merkle_root(&mut ledger), root);
    assert_eq!(ledger.num_accounts(), 1);
}

// Failed zkApp commands.

/// Bob, Carol and Dave each get 1 MINA from Alice in one command, with a
/// nonce precondition that can't hold at account update `failing` (none
/// for 0). The updates are Alice's debit, then the three credits.
fn three_credits(failing: usize) -> (Mask, Vec<CompressedPubKey>, Transaction) {
    let keys: Vec<_> = (0..4).map(|_| key()).collect();
    let ledger = ledger(&[
        (&keys[0], 10 * MINA),
        (&keys[1], MINA),
        (&keys[2], MINA),
        (&keys[3], MINA),
    ]);
    let mut debit = body(&keys[0]);
    debit.balance_change = amount(Sgn::Neg, 3 * MINA);
    let mut updates = vec![signed(debit)];
    for pk in &keys[1..] {
        let mut body = body(pk);
        body.balance_change = amount(Sgn::Pos, MINA);
        updates.push(none_given(body));
    }
    if failing > 0 {
        let body = &mut updates[failing - 1].body;
        body.preconditions.account.0.nonce = OrIgnore::Check(ClosedInterval {
            lower: Nonce::from_u32(7),
            upper: Nonce::from_u32(7),
        });
    }
    let command = command(&keys[0], 0, updates);
    (ledger, keys, zkapp(command))
}

/// A command failing at its first, a middle or its last account update:
/// the fee is charged and the nonce taken, every other change is dropped,
/// and the failure is in the update's own entry of the table, the others
/// `Cancelled`, as OCaml's `apply_zkapp_command_unchecked_aux` reports it
/// (the fee payer first).
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/transaction_logic/mina_transaction_logic.ml#L1517-L1529>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/transaction_logic/mina_transaction_logic.ml#L1801-L1840>
#[test]
fn a_failed_command_charges_its_fee_and_drops_everything_else() {
    // Applied, for reference.
    let (mut ledger, keys, txn) = three_credits(0);
    let applied = apply(&mut ledger, 10, &[txn]);
    assert!(status(&applied[0]).is_applied());
    for pk in &keys[1..] {
        assert_eq!(
            account(&ledger, pk).unwrap().balance,
            Balance::from_u64(2 * MINA)
        );
    }

    // Updates 1 to 4: Alice's debit, then the three credits.
    for failing in 1..=4 {
        let (mut ledger, keys, txn) = three_credits(failing);
        let mut before: Vec<_> = keys
            .iter()
            .map(|pk| account(&ledger, pk).unwrap())
            .collect();
        let applied = apply(&mut ledger, 10, &[txn]);
        assert_eq!(
            failures(&applied[0]),
            only_failure(
                4,
                failing,
                TransactionFailure::AccountNoncePreconditionUnsatisfied
            ),
            "failing at {failing}"
        );
        // Alice pays the fee and takes the nonce; nothing else changes.
        let alice = &mut before[0];
        alice.balance = alice.balance.sub_amount(Amount::from_u64(FEE)).unwrap();
        alice.nonce = alice.nonce.incr();
        let after: Vec<_> = keys
            .iter()
            .map(|pk| account(&ledger, pk).unwrap())
            .collect();
        assert_eq!(after[0].balance, before[0].balance, "failing at {failing}");
        assert_eq!(after[0].nonce, before[0].nonce, "failing at {failing}");
        for (after, before) in after.iter().zip(&before).skip(1) {
            assert_eq!(after, before, "failing at {failing}");
        }
    }
}

/// The command's local state keeps its failure, and the ledger it builds,
/// until the last account update, where both are reset: OCaml resets
/// `success` and `ledger` only when `is_last_account_update`.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/transaction_logic/zkapp_command_logic.ml#L1964-L2020>
#[test]
fn a_failed_command_resets_its_local_state_only_at_the_last_update() {
    for failing in 1..=4 {
        let (ledger, keys, txn) = three_credits(failing);
        let Transaction::Command(UserCommand::ZkAppCommand(command)) = txn else {
            unreachable!()
        };
        let ids: Vec<_> = keys.iter().map(id).collect();
        let first = SparseLedger::of_ledger_subset_exn(ledger, &ids);
        let mut states = vec![];
        let mut after_fee = first.copy_content();
        let partial = after_fee
            .apply_zkapp_first_pass_unchecked_with_states(
                &mut states,
                Slot::from_u32(10),
                &view(10),
                Signed::zero(),
                Signed::zero(),
                &first,
                &command,
            )
            .unwrap();
        let applied = after_fee
            .copy_content()
            .apply_zkapp_second_pass_unchecked_with_states(&mut states, partial)
            .unwrap();
        assert!(!applied.command.status.is_applied());

        let success: Vec<bool> = states.iter().map(|(_, local)| local.success).collect();
        let (last, before_last) = success.split_last().unwrap();
        assert!(
            *last,
            "reset at the last update, failing at {failing}: {success:?}"
        );
        // Once failed, failed until the last update; a failure at the last
        // update is reset in the same step.
        let first_failed = before_last.iter().position(|success| !success);
        if failing < 4 {
            let first_failed = first_failed.expect("a failed state before the last");
            assert!(
                before_last[first_failed..].iter().all(|s| !s),
                "{success:?}"
            );
        } else {
            assert_eq!(first_failed, None, "{success:?}");
        }
    }
}

// Verification key changes within one command.

fn vk(max_proofs_verified: ProofVerified) -> VerificationKeyWire {
    let mut vk = (*VerificationKey::dummy()).clone();
    vk.max_proofs_verified = max_proofs_verified;
    VerificationKeyWire::new(vk)
}

/// Bob's zkApp, with key `vk` and these permissions.
fn zkapp_account(
    ledger: &mut Mask,
    bob: &CompressedPubKey,
    vk: &VerificationKeyWire,
    permissions: Permissions<AuthRequired>,
) {
    let mut account = *account(ledger, bob).unwrap();
    account.permissions = permissions;
    account.zkapp = Some(Box::new(ZkAppAccount {
        verification_key: Some(vk.clone()),
        ..ZkAppAccount::default()
    }));
    set_account(ledger, account);
}

fn permissions(
    edit_state: AuthRequired,
    set_verification_key: AuthRequired,
    set_permissions: AuthRequired,
) -> Permissions<AuthRequired> {
    Permissions {
        edit_state,
        set_verification_key: SetVerificationKey {
            auth: set_verification_key,
            txn_version: TXN_VERSION_CURRENT,
        },
        set_permissions,
        ..Permissions::user_default()
    }
}

fn set_vk(pk: &CompressedPubKey, vk: &VerificationKeyWire) -> Body {
    let mut body = body(pk);
    body.update.verification_key = SetOrKeep::Set(vk.clone());
    body
}

fn set_state(pk: &CompressedPubKey, fields: usize, value: u64) -> Body {
    let mut body = body(pk);
    for field in body.update.app_state.iter_mut().take(fields) {
        *field = SetOrKeep::Set(Fp::from(value));
    }
    body
}

fn zkapp_of(ledger: &Mask, pk: &CompressedPubKey) -> ZkAppAccount {
    *account(ledger, pk).unwrap().zkapp.unwrap()
}

/// Swap the key, change the state with a proof against the new key, swap
/// it back: applied, with the first key and the new state, and
/// `proved_state` set by the proof that replaced the whole state and kept
/// by the signed update that left the state alone.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/transaction_logic/zkapp_command_logic.ml#L1308-L1317>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/transaction_logic/zkapp_command_logic.ml#L1569-L1645>
#[test]
fn a_key_swapped_and_back_in_one_command() {
    let (vk1, vk2) = (vk(ProofVerified::N2), vk(ProofVerified::N1));
    assert_ne!(vk1.hash(), vk2.hash());
    let (alice, bob) = (key(), key());
    let run = |updates: Vec<AccountUpdate>, permissions: Permissions<AuthRequired>| {
        let mut ledger = ledger(&[(&alice, 10 * MINA), (&bob, MINA)]);
        zkapp_account(&mut ledger, &bob, &vk1, permissions);
        let applied = apply(&mut ledger, 10, &[zkapp(command(&alice, 0, updates))]);
        (failures(&applied[0]), zkapp_of(&ledger, &bob))
    };
    let guarded = || {
        permissions(
            AuthRequired::Proof,
            AuthRequired::Signature,
            AuthRequired::Proof,
        )
    };

    let (failed, zkapp) = run(
        vec![
            signed(set_vk(&bob, &vk2)),
            proved(set_state(&bob, ZKAPP_STATE_SIZE, 5), vk2.hash()),
            signed(set_vk(&bob, &vk1)),
        ],
        guarded(),
    );
    assert_eq!(failed, Vec::<Vec<_>>::new());
    assert_eq!(zkapp.verification_key.unwrap().hash(), vk1.hash());
    assert_eq!(zkapp.app_state, [Fp::from(5u64); ZKAPP_STATE_SIZE]);
    assert!(zkapp.proved_state);

    // A proof against the key the account had before the command: its
    // key is the new one by then.
    let (failed, zkapp) = run(
        vec![
            signed(set_vk(&bob, &vk2)),
            proved(set_state(&bob, ZKAPP_STATE_SIZE, 5), vk1.hash()),
            signed(set_vk(&bob, &vk1)),
        ],
        guarded(),
    );
    assert_eq!(
        failed,
        only_failure(3, 2, TransactionFailure::UnexpectedVerificationKeyHash)
    );
    assert_eq!(zkapp.verification_key.unwrap().hash(), vk1.hash());
    assert_eq!(zkapp.app_state, [Fp::zero(); ZKAPP_STATE_SIZE]);

    // A proof that replaces part of the state leaves `proved_state`; a
    // signed update that changes the state clears it.
    let either = || {
        permissions(
            AuthRequired::Either,
            AuthRequired::Signature,
            AuthRequired::Proof,
        )
    };
    let (failed, zkapp) = run(
        vec![
            signed(set_vk(&bob, &vk2)),
            proved(set_state(&bob, ZKAPP_STATE_SIZE, 5), vk2.hash()),
            proved(set_state(&bob, 3, 6), vk2.hash()),
        ],
        either(),
    );
    assert_eq!(failed, Vec::<Vec<_>>::new());
    assert!(zkapp.proved_state);
    let (failed, zkapp) = run(
        vec![
            signed(set_vk(&bob, &vk2)),
            proved(set_state(&bob, ZKAPP_STATE_SIZE, 5), vk2.hash()),
            signed(set_state(&bob, 1, 6)),
        ],
        either(),
    );
    assert_eq!(failed, Vec::<Vec<_>>::new());
    assert!(!zkapp.proved_state);
}

/// `set_verification_key` weaker than `set_permissions`: a signature swaps
/// in a new key, and a proof against it then passes a `Proof`
/// `set_permissions`. OCaml allows it; so do we.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/transaction_logic/zkapp_command_logic.ml#L1622-L1645>
#[test]
fn a_signed_key_swap_then_proved_permissions() {
    let (vk1, vk2) = (vk(ProofVerified::N2), vk(ProofVerified::N1));
    let (alice, bob) = (key(), key());
    let mut ledger = ledger(&[(&alice, 10 * MINA), (&bob, MINA)]);
    zkapp_account(
        &mut ledger,
        &bob,
        &vk1,
        permissions(
            AuthRequired::Proof,
            AuthRequired::Either,
            AuthRequired::Proof,
        ),
    );
    let open = permissions(AuthRequired::None, AuthRequired::None, AuthRequired::None);
    let mut set_permissions = body(&bob);
    set_permissions.update.permissions = SetOrKeep::Set(open.clone());
    let applied = apply(
        &mut ledger,
        10,
        &[zkapp(command(
            &alice,
            0,
            vec![
                signed(set_vk(&bob, &vk2)),
                proved(set_permissions.clone(), vk2.hash()),
            ],
        ))],
    );
    assert_eq!(failures(&applied[0]), Vec::<Vec<_>>::new());
    assert_eq!(account(&ledger, &bob).unwrap().permissions, open);

    // Signed, the same permissions change fails `set_permissions`.
    let mut ledger2 = self::ledger(&[(&alice, 10 * MINA), (&bob, MINA)]);
    zkapp_account(
        &mut ledger2,
        &bob,
        &vk1,
        permissions(
            AuthRequired::Proof,
            AuthRequired::Either,
            AuthRequired::Proof,
        ),
    );
    let applied = apply(
        &mut ledger2,
        10,
        &[zkapp(command(&alice, 0, vec![signed(set_permissions)]))],
    );
    assert_eq!(
        failures(&applied[0]),
        only_failure(1, 1, TransactionFailure::UpdateNotPermittedPermissions)
    );
}

/// An account update's own permission change applies after its key change:
/// relaxing `set_verification_key` and changing the key in one signed
/// update fails, in two updates it is applied. OCaml sets the key before
/// the permissions.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/transaction_logic/zkapp_command_logic.ml#L1622-L1645>
#[test]
fn a_permission_change_applies_after_the_key_change_of_its_update() {
    let (vk1, vk2) = (vk(ProofVerified::N2), vk(ProofVerified::N1));
    let (alice, bob) = (key(), key());
    let locked = || {
        permissions(
            AuthRequired::Signature,
            AuthRequired::Proof,
            AuthRequired::Signature,
        )
    };
    let relaxed = permissions(
        AuthRequired::Signature,
        AuthRequired::Signature,
        AuthRequired::Signature,
    );
    let run = |updates: Vec<AccountUpdate>| {
        let mut ledger = ledger(&[(&alice, 10 * MINA), (&bob, MINA)]);
        zkapp_account(&mut ledger, &bob, &vk1, locked());
        let applied = apply(&mut ledger, 10, &[zkapp(command(&alice, 0, updates))]);
        (failures(&applied[0]), zkapp_of(&ledger, &bob))
    };

    let mut both = set_vk(&bob, &vk2);
    both.update.permissions = SetOrKeep::Set(relaxed.clone());
    let (failed, zkapp) = run(vec![signed(both)]);
    assert_eq!(
        failed,
        only_failure(1, 1, TransactionFailure::UpdateNotPermittedVerificationKey)
    );
    assert_eq!(zkapp.verification_key.unwrap().hash(), vk1.hash());

    let mut relax = body(&bob);
    relax.update.permissions = SetOrKeep::Set(relaxed);
    let (failed, zkapp) = run(vec![signed(relax), signed(set_vk(&bob, &vk2))]);
    assert_eq!(failed, Vec::<Vec<_>>::new());
    assert_eq!(zkapp.verification_key.unwrap().hash(), vk2.hash());
}

/// The keys a command's proofs are checked against: a key set by an
/// earlier account update of the same account, not the ledger's, and the
/// ledger's before any. A proof against the ledger's key after a swap is
/// rejected. OCaml's `Verifiable.create` tracks `vks_overridden` the same way.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/mina_base/zkapp_command.ml#L546-L630>
#[test]
fn proofs_are_checked_against_the_key_set_earlier_in_the_command() {
    let (vk1, vk2) = (vk(ProofVerified::N2), vk(ProofVerified::N1));
    let (alice, bob) = (key(), key());
    let find_vk = |hash: Fp, account: &AccountId| {
        if account == &id(&bob) && hash == vk1.hash() {
            Ok(vk1.clone())
        } else {
            Err("no such key".to_string())
        }
    };
    let keys = |updates: Vec<AccountUpdate>| {
        verifiable::create(&command(&alice, 0, updates), false, find_vk).map(|command| {
            command
                .account_updates
                .to_account_updates()
                .into_iter()
                .map(|(_, vk)| vk.map(|vk| vk.hash()))
                .collect::<Vec<_>>()
        })
    };

    assert_eq!(
        keys(vec![
            proved(body(&bob), vk1.hash()),
            signed(set_vk(&bob, &vk2)),
            proved(body(&bob), vk2.hash()),
            signed(set_vk(&bob, &vk1)),
            proved(body(&bob), vk1.hash()),
        ])
        .unwrap(),
        vec![
            Some(vk1.hash()),
            None,
            Some(vk2.hash()),
            None,
            Some(vk1.hash())
        ]
    );
    // A proof that sets the key is checked against the key before it.
    assert_eq!(
        keys(vec![proved(set_vk(&bob, &vk2), vk1.hash())]).unwrap(),
        vec![Some(vk1.hash())]
    );
    assert!(keys(vec![
        signed(set_vk(&bob, &vk2)),
        proved(body(&bob), vk1.hash())
    ])
    .is_err());
}

// `valid_while`.

/// A command whose one account update holds only while the global slot is
/// in `[lower, upper]`, applied at `slot`. Applied or the failure table.
fn valid_while(lower: u32, upper: u32, slot: u32) -> Vec<Vec<TransactionFailure>> {
    let (alice, bob) = (key(), key());
    let mut ledger = ledger(&[(&alice, 10 * MINA), (&bob, MINA)]);
    let mut update = body(&bob);
    update.preconditions.valid_while = OrIgnore::Check(ClosedInterval {
        lower: Slot::from_u32(lower),
        upper: Slot::from_u32(upper),
    });
    let applied = apply(
        &mut ledger,
        slot,
        &[zkapp(command(&alice, 0, vec![none_given(update)]))],
    );
    failures(&applied[0])
}

/// `valid_while` holds from its lower to its upper bound, both included,
/// against the block's global slot, as OCaml's `Closed_interval` check.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/mina_base/zkapp_precondition.ml#L267-L273>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/transaction_logic/zkapp_command_logic.ml#L1335-L1345>
#[test]
fn valid_while_bounds_are_inclusive() {
    let ok = Vec::<Vec<TransactionFailure>>::new();
    let failed = only_failure(1, 1, TransactionFailure::ValidWhilePreconditionUnsatisfied);
    let max = u32::MAX;
    let cases = [
        // Single slot.
        (1000, 1000, 1000, true),
        (1000, 1000, 999, false),
        (1000, 1000, 1001, false),
        // The bounds themselves, and one past them.
        (900, 1000, 900, true),
        (900, 1000, 1000, true),
        (900, 1000, 899, false),
        (900, 1000, 1001, false),
        // Touching ranges: their shared slot is in both.
        (500, 1000, 1000, true),
        (1000, 1500, 1000, true),
        // Inverted: never.
        (1001, 999, 1000, false),
        (1000, 999, 1000, false),
        (1000, 999, 999, false),
        // Maximum values.
        (0, max, max, true),
        (max, max, max, true),
        (max - 1, max - 1, max, false),
        (0, max, 0, true),
    ];
    for (lower, upper, slot, holds) in cases {
        let want = if holds { &ok } else { &failed };
        assert_eq!(
            &valid_while(lower, upper, slot),
            want,
            "[{lower}, {upper}] at {slot}"
        );
    }
}
