//! Pool-level ports of Mina 4.0.2 tests. Slots and account snapshots are supplied
//! explicitly: no wall-clock sleeps, node, frontier pipes or proof generation.
//! See private implementation notes for the source map and harness boundaries.
#![allow(clippy::mutable_key_type)]

use super::*;
use crate::{
    account::AuthRequired,
    scan_state::{
        currency::{Sgn, Signed},
        transaction_logic::{
            signed_command::{
                Body as PaymentBody, PaymentPayload, SignedCommand, SignedCommandPayload,
            },
            transaction_union_payload::TransactionUnionPayload,
            zkapp_command::{self as zk, AccountUpdate, CallForest},
            Memo,
        },
    },
    ToInputs,
};
use mina_signer::{Keypair, NetworkId, NonceMode, Signer};
use rand::{rngs::StdRng, Rng, SeedableRng};
use std::sync::LazyLock;

#[path = "indexed_tests.rs"]
mod indexed;
#[path = "resource_tests.rs"]
mod resource;

const MINA: u64 = 1_000_000_000;
// OCaml's transaction_pool tests use 2 MINA, not the mainnet minimum.
const FEE: u64 = 2 * MINA;
const BALANCE: u64 = 900_000_000 * MINA;

static KEYS: LazyLock<Vec<Keypair>> = LazyLock::new(|| {
    let mut rng = StdRng::seed_from_u64(0x503664);
    (0..16).map(|_| Keypair::rand(&mut rng).unwrap()).collect()
});

fn id(i: usize) -> AccountId {
    AccountId::new(KEYS[i].public.into_compressed(), TokenId::default())
}

fn constants() -> ConsensusConstants {
    ConsensusConstants {
        k: 290,
        delta: 0,
        block_window_duration_ms: 180_000,
        slots_per_sub_window: 7,
        slots_per_window: 77,
        sub_windows_per_window: 11,
        slots_per_epoch: 7140,
        grace_period_slots: 2160,
        grace_period_end: 2237,
        slot_duration_ms: 180_000,
        epoch_duration: 7140 * 180_000,
        checkpoint_window_slots_per_year: 175200,
        checkpoint_window_size_in_slots: 14600,
        delta_duration: 180_000,
        genesis_state_timestamp: (&BlockTime::from_u64(0)).into(),
    }
}

fn accounts(balance: u64) -> BTreeMap<AccountId, Account> {
    (0..16)
        .map(|i| {
            let id = id(i);
            (
                id.clone(),
                Account::create_with(id, Balance::from_u64(balance)),
            )
        })
        .collect()
}

fn payment(sender: usize, nonce: u32, amount: u64, fee: u64) -> ValidCommandWithHash {
    payment_until(sender, nonce, amount, fee, u32::MAX)
}

fn payment_until(
    sender: usize,
    nonce: u32,
    amount: u64,
    fee: u64,
    until: u32,
) -> ValidCommandWithHash {
    let kp = &KEYS[sender];
    let payload = SignedCommandPayload::create(
        Fee::from_u64(fee),
        kp.public.into_compressed(),
        Nonce::from_u32(nonce),
        Some(Slot::from_u32(until)),
        Memo::empty(),
        PaymentBody::Payment(PaymentPayload {
            receiver_pk: KEYS[15].public.into_compressed(),
            amount: Amount::from_u64(amount),
        }),
    );
    let signature = mina_signer::create_legacy(NetworkId::MAINNET).sign(
        kp,
        &TransactionUnionPayload::of_user_command_payload(&payload),
        NonceMode::Legacy,
    );
    transaction_hash::hash_command(valid::UserCommand::SignedCommand(Box::new(SignedCommand {
        payload,
        signer: kp.public.into_compressed(),
        signature,
    })))
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/test/indexed_pool_tests.ml#L512-L585>
fn zkapp(
    sender: usize,
    receiver: usize,
    nonce: u32,
    fee: u64,
    double: bool,
    increment_receiver: bool,
) -> ValidCommandWithHash {
    let update = |who: usize, increment: bool, negative: bool| AccountUpdate {
        body: zk::Body {
            public_key: KEYS[who].public.into_compressed(),
            token_id: TokenId::default(),
            update: zk::Update::noop(),
            balance_change: Signed {
                magnitude: Amount::from_u64(MINA),
                sgn: if negative { Sgn::Neg } else { Sgn::Pos },
            },
            increment_nonce: increment,
            events: zk::Events::empty(),
            actions: zk::Actions::empty(),
            call_data: Fp::from(0),
            preconditions: zk::Preconditions {
                network: zk::ZkAppPreconditions::accept(),
                account: zk::AccountPreconditions(zk::Account::accept()),
                valid_while: zk::Numeric::Ignore,
            },
            use_full_commitment: true,
            implicit_account_creation_fee: false,
            may_use_token: zk::MayUseToken::No,
            authorization_kind: zk::AuthorizationKind::NoneGiven,
        },
        authorization: zk::Control::NoneGiven,
    };
    let forest = CallForest::empty()
        .cons(None, update(receiver, increment_receiver, false))
        .cons(None, update(sender, double, true));
    let cmd = zk::ZkAppCommand {
        fee_payer: zk::FeePayer {
            body: zk::FeePayerBody {
                public_key: KEYS[sender].public.into_compressed(),
                fee: Fee::from_u64(fee),
                valid_until: None,
                nonce: Nonce::from_u32(nonce),
            },
            authorization: mina_signer::Signature::dummy(),
        },
        account_updates: forest,
        memo: Memo::empty(),
    };
    sign_zkapp(cmd, sender)
}

// Rebuild caches after fixture mutations, with explicit mainnet domain
// parameters. Other unit tests in this binary initialize the global to devnet.
fn mainnet_forest(forest: &CallForest<AccountUpdate>) -> CallForest<AccountUpdate> {
    use poseidon::hash::{
        hash_with_kimchi,
        params::{MAINNET_ZKAPP_BODY, MINA_ACCOUNT_UPDATE_CONS},
    };
    let mut tail = Fp::from(0);
    let mut result = Vec::new();
    for node in forest.0.iter().rev() {
        let update = node.elt.account_update.clone();
        let tree = zk::Tree {
            account_update_digest: crate::MutableFp::new(
                update.hash_with_param(&MAINNET_ZKAPP_BODY),
            ),
            account_update: update,
            calls: mainnet_forest(&node.elt.calls),
        };
        tail = hash_with_kimchi(&MINA_ACCOUNT_UPDATE_CONS, &[tree.digest(), tail]);
        result.push(zk::WithStackHash {
            elt: tree,
            stack_hash: crate::MutableFp::new(tail),
        });
    }
    result.reverse();
    CallForest(result)
}

fn sign_zkapp(mut cmd: zk::ZkAppCommand, sender: usize) -> ValidCommandWithHash {
    cmd.account_updates = mainnet_forest(&cmd.account_updates);
    let full = cmd.commitment().create_complete(
        cmd.memo.hash(),
        AccountUpdate::of_fee_payer(cmd.fee_payer.clone())
            .hash_with_param(&poseidon::hash::params::MAINNET_ZKAPP_BODY),
    );
    cmd.fee_payer.authorization = mina_signer::create_kimchi(NetworkId::MAINNET).sign(
        &KEYS[sender],
        &full,
        NonceMode::Chunked,
    );
    // Pool tests start after signature/proof verification, just like OCaml's
    // to_valid_unsafe helper. Fee-payer signatures are nevertheless real.
    transaction_hash::hash_command(UserCommand::ZkAppCommand(Box::new(cmd)).to_valid_unsafe())
}

fn alter_zkapp(
    cmd: &ValidCommandWithHash,
    sender: usize,
    f: impl FnOnce(&mut zk::ZkAppCommand),
) -> ValidCommandWithHash {
    let UserCommand::ZkAppCommand(mut cmd) = cmd.data.forget_check() else {
        panic!("expected zkApp")
    };
    f(&mut cmd);
    sign_zkapp(*cmd, sender)
}

fn independent(zkapp_kind: bool) -> Vec<ValidCommandWithHash> {
    (0..7)
        .map(|i| {
            if zkapp_kind {
                zkapp(i, 15, 0, FEE, false, false)
            } else {
                payment(i, 0, MINA, FEE)
            }
        })
        .collect()
}

fn hashes<'a>(
    cmds: impl IntoIterator<Item = &'a ValidCommandWithHash>,
) -> BTreeSet<v2::TransactionHash> {
    cmds.into_iter().map(|c| c.hash.clone()).collect()
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/indexed_pool.ml#L109-L216>
fn assert_pool_consistency(pool: &IndexedPool) {
    let sender = hashes(pool.all_by_sender.values().flat_map(|(q, _)| q));
    let fee = hashes(pool.all_by_fee.values().flatten());
    let hash = hashes(pool.all_by_hash.values());
    assert_eq!(pool.size, hash.len());
    assert_eq!(sender, hash);
    assert_eq!(fee, hash);
    assert_eq!(
        hashes(pool.all_by_sender.values().map(|(q, _)| q.front().unwrap())),
        hashes(pool.applicable_by_fee.values().flatten())
    );
    for (id, (queue, reserved)) in &pool.all_by_sender {
        assert!(!queue.is_empty());
        let mut last = None;
        let mut sum = 0u64;
        for cmd in queue {
            let c = cmd.data.forget_check();
            assert_eq!(&c.fee_payer(), id);
            let nonce = c.applicable_at_nonce();
            assert!(last.is_none_or(|last| nonce > last));
            last = Some(nonce);
            // Independent arithmetic, not currency_consumed from the port.
            sum = sum.checked_add(c.fee().as_u64()).unwrap();
            if let UserCommand::SignedCommand(c) = c {
                if let PaymentBody::Payment(p) = &c.payload.body {
                    sum = sum.checked_add(p.amount.as_u64()).unwrap();
                }
            }
        }
        assert_eq!(reserved.as_u64(), sum);
    }
    for map in [&pool.applicable_by_fee, &pool.all_by_fee] {
        for (fee, cmds) in map {
            assert!(!cmds.is_empty());
            for c in cmds {
                assert_eq!(fee, &c.data.forget_check().fee_per_wu());
            }
        }
    }
    for (hash, c) in &pool.all_by_hash {
        assert_eq!(hash, &c.hash);
    }
    let expiring = hashes(
        pool.all_by_hash
            .values()
            .filter(|c| c.data.forget_check().valid_until() != Slot::max()),
    );
    assert_eq!(
        expiring,
        hashes(pool.transactions_with_expiration.values().flatten())
    );
    for (slot, cmds) in &pool.transactions_with_expiration {
        assert!(!cmds.is_empty());
        for c in cmds {
            assert_eq!(*slot, c.data.forget_check().valid_until());
        }
    }
}

fn add(
    pool: &mut IndexedPool,
    cmd: &ValidCommandWithHash,
    nonce: u32,
    balance: u64,
) -> Result<VecDeque<ValidCommandWithHash>, CommandError> {
    let result = pool
        .add_from_gossip_exn(
            Slot::zero(),
            Slot::zero(),
            cmd,
            Nonce::from_u32(nonce),
            Balance::from_u64(balance),
        )
        .map(|(_, dropped)| dropped);
    assert_pool_consistency(pool);
    result
}

struct Harness {
    pool: TransactionPool,
    accounts: BTreeMap<AccountId, Account>,
    slot: u32,
}

impl Harness {
    fn new() -> Self {
        Self {
            pool: TransactionPool::new(
                Config {
                    trust_system: (),
                    pool_max_size: 25,
                    slot_tx_end: None,
                },
                &constants(),
            ),
            accounts: accounts(BALANCE),
            slot: 0,
        }
    }

    fn submit(
        &mut self,
        cmds: &[ValidCommandWithHash],
        local: bool,
    ) -> (
        Vec<ValidCommandWithHash>,
        Vec<(ValidCommandWithHash, diff::Error)>,
    ) {
        let (_, accepted, rejected, _) = self
            .pool
            .unsafe_apply(
                redux::Timestamp::ZERO,
                Slot::from_u32(self.slot),
                Slot::from_u32(self.slot),
                &diff::DiffVerified {
                    list: cmds.to_vec(),
                },
                &self.accounts,
                local,
            )
            .unwrap();
        assert_pool_consistency(&self.pool.pool);
        (accepted, rejected)
    }

    fn add(&mut self, cmds: &[ValidCommandWithHash], local: bool) {
        let (accepted, rejected) = self.submit(cmds, local);
        assert_eq!(hashes(&accepted), hashes(cmds));
        assert!(rejected.is_empty(), "{rejected:?}");
    }

    fn assert_txs(&self, expected: &[ValidCommandWithHash]) {
        assert_eq!(hashes(&self.pool.get_all_transactions()), hashes(expected));
        assert_pool_consistency(&self.pool.pool);
    }

    fn diff(&mut self, new: &[ValidCommandWithHash], removed: &[ValidCommandWithHash]) {
        let with_status = |cs: &[ValidCommandWithHash]| {
            cs.iter()
                .map(|c| WithStatus {
                    data: c.data.clone(),
                    status: Applied,
                })
                .collect()
        };
        let diff = diff::BestTipDiff {
            new_commands: with_status(new),
            removed_commands: with_status(removed),
            reorg_best_tip: !removed.is_empty(),
        };
        let (ids, _) = self.pool.get_accounts_to_handle_transition_diff(&diff);
        self.pool
            .handle_transition_frontier_diff(
                Slot::from_u32(self.slot),
                Slot::from_u32(self.slot),
                &diff,
                &ids,
                &self.accounts,
                &self.accounts,
            )
            .unwrap();
        assert_pool_consistency(&self.pool.pool);
    }

    // The pool's input is a post-block account snapshot. Model only the transfer
    // and nonce effects used by these fixtures, independently of pool routines.
    fn commit(&mut self, cmds: &[ValidCommandWithHash]) {
        apply_to_accounts(&mut self.accounts, cmds);
        self.diff(cmds, &[]);
    }

    fn assert_rebroadcastable(&mut self, expected: &[ValidCommandWithHash]) {
        let actual = self.pool.get_rebroadcastable(|_| false);
        assert!(actual.len() <= 1);
        let actual: Vec<_> = actual
            .into_iter()
            .flatten()
            .map(|c| transaction_hash::hash_command(c.to_valid_unsafe()))
            .collect();
        assert_eq!(hashes(&actual), hashes(expected));
    }
}

fn apply_to_accounts(accounts: &mut BTreeMap<AccountId, Account>, cmds: &[ValidCommandWithHash]) {
    for cmd in cmds {
        let c = cmd.data.forget_check();
        let a = accounts.get_mut(&c.fee_payer()).unwrap();
        a.nonce = a.nonce.succ();
        a.balance = Balance::from_u64(a.balance.as_u64().checked_sub(c.fee().as_u64()).unwrap());
        match c {
            UserCommand::SignedCommand(c) => {
                if let PaymentBody::Payment(p) = &c.payload.body {
                    a.balance = Balance::from_u64(
                        a.balance.as_u64().checked_sub(p.amount.as_u64()).unwrap(),
                    );
                    let receiver = accounts
                        .get_mut(&AccountId::new(p.receiver_pk.clone(), TokenId::default()))
                        .unwrap();
                    receiver.balance = Balance::from_u64(
                        receiver
                            .balance
                            .as_u64()
                            .checked_add(p.amount.as_u64())
                            .unwrap(),
                    );
                }
            }
            UserCommand::ZkAppCommand(c) => {
                for u in c.account_updates.to_account_updates() {
                    let a = accounts
                        .get_mut(&AccountId::new(
                            u.body.public_key.clone(),
                            u.body.token_id.clone(),
                        ))
                        .unwrap();
                    if u.body.increment_nonce {
                        a.nonce = a.nonce.succ();
                    }
                    let amount = u.body.balance_change.magnitude.as_u64();
                    a.balance = Balance::from_u64(
                        match u.body.balance_change.sgn {
                            Sgn::Pos => a.balance.as_u64().checked_add(amount),
                            Sgn::Neg => a.balance.as_u64().checked_sub(amount),
                        }
                        .unwrap(),
                    );
                    if let zk::SetOrKeep::Set(perms) = &u.body.update.permissions {
                        a.permissions = perms.clone();
                    }
                }
            }
        }
    }
}
