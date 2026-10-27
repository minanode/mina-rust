//! Root commits use the actual scan-state reference traversal. The synthetic
//! jobs below test retention, not transaction or proof verification.

use super::*;
use ledger::{
    scan_state::{
        currency::{Balance, Fee},
        scan_state::{
            transaction_snark::{LedgerProof, OneOrTwo, TransactionWithWitness},
            ScanState,
        },
        transaction_logic::{FeeTransfer, SingleFeeTransfer, TransactionStatus, WithStatus},
    },
    AccountIndex,
};

pub(super) fn pending_scan(accounts: impl IntoIterator<Item = Account>) -> ScanState {
    // The unused statement is from captured OCaml work, with empty witnesses.
    // Only the transactions' referenced accounts are under test here.
    let bytes = include_bytes!("../../../ledger/src/verifier/fixtures/mainnet-551267-1.bin");
    let work = v2::TransactionSnarkWorkTStableV2::binprot_read(&mut bytes.as_slice()).unwrap();
    let wire = match &work.proofs {
        v2::TransactionSnarkWorkTStableV2Proofs::One(p) => p,
        v2::TransactionSnarkWorkTStableV2Proofs::Two((p, _)) => p,
    };
    let statement = LedgerProof::try_from(wire).unwrap().statement();
    let jobs = accounts
        .into_iter()
        .map(|account| {
            Arc::new(TransactionWithWitness {
                transaction_with_status: WithStatus {
                    data: Transaction::FeeTransfer(
                        FeeTransfer::of_singles(OneOrTwo::One(SingleFeeTransfer::create(
                            account.public_key,
                            Fee::from_u64(1),
                            account.token_id,
                        )))
                        .unwrap(),
                    ),
                    status: TransactionStatus::Applied,
                },
                state_hash: (Fp::from(0), Fp::from(0)),
                init_stack: statement.source.pending_coinbase_stack.clone(),
                statement: statement.clone(),
                first_pass_ledger_witness: SparseLedger::create(LEDGER_DEPTH, Fp::from(0)),
                second_pass_ledger_witness: SparseLedger::create(LEDGER_DEPTH, Fp::from(0)),
                block_global_slot: Slot::from_u32(0),
                hash_cache: Default::default(),
            })
        })
        .collect();
    let mut scan = ScanState::empty(constraint_constants());
    scan.scan_state.update(jobs, vec![], |_| 0).unwrap();
    scan
}

fn insert_staged(
    ctx: &mut LedgerCtx,
    mask: Mask,
    scan: ScanState,
) -> MinaBaseStagedLedgerHashStableV1 {
    let empty = StagedLedger::create_exn(constraint_constants().clone(), mask.clone()).unwrap();
    let mut staged = StagedLedger::from_persisted_unchecked(
        constraint_constants(),
        scan,
        mask,
        empty.pending_coinbase_collection().clone(),
    );
    let hash = MinaBaseStagedLedgerHashStableV1::from(&staged.hash());
    ctx.staged_ledgers.insert(Arc::new(hash.clone()), staged);
    hash
}

#[test]
fn root_commit_keeps_equal_pending_then_reclaims_finalized_accounts() {
    for disk in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut snarked = Mask::create(LEDGER_DEPTH);
        let account = Account::create_with(
            AccountId::new(Account::rand().public_key, TokenId::default()),
            Balance::from_u64(100),
        );
        snarked
            .get_or_create_account(account.id(), account.clone())
            .unwrap();
        let snarked_hash = merkle_root(&mut snarked);
        let staged = snarked.make_child();
        staged.hold_parent_accounts([account.id()]);
        let mut ctx = LedgerCtx::default();
        ctx.snarked_ledgers
            .insert(snarked_hash.clone(), snarked.clone());
        let hash = insert_staged(&mut ctx, staged.clone(), pending_scan([account.clone()]));
        let root = staged.clone().merkle_root();
        if disk {
            snarked.freeze_to_disk(dir.path()).unwrap().unwrap();
        }
        assert!(ctx.commit_root_staged_ledger(&hash, &snarked_hash));
        assert_eq!(staged.num_own_accounts(), 1, "pending equal account");
        assert_eq!(staged.clone().merkle_root(), root);
        ctx.staged_ledgers.take();
        let hash = insert_staged(
            &mut ctx,
            staged.clone(),
            ScanState::empty(constraint_constants()),
        );
        assert!(ctx.commit_root_staged_ledger(&hash, &snarked_hash));
        assert_eq!(staged.num_own_accounts(), 0, "fully emitted account");
        assert_eq!(staged.clone().merkle_root(), root);
        if disk {
            assert_eq!(
                snarked.disk_cached(),
                Some(0),
                "commit did not read account records"
            );
        }
    }
}

#[test]
fn root_staged_ownership_is_bounded_by_pending_window() {
    const ROUNDS: usize = 2048;
    const PER_ROUND: usize = 2;
    const WINDOW: usize = 4;
    let mut snarked = Mask::create(LEDGER_DEPTH);
    let accounts: Vec<_> = (0..ROUNDS * PER_ROUND)
        .map(|_| {
            let mut account = Account::rand();
            account.token_id = TokenId::default();
            account.balance = Balance::from_u64(100);
            snarked
                .get_or_create_account(account.id(), account.clone())
                .unwrap();
            account.balance = Balance::from_u64(101);
            account
        })
        .collect();
    snarked.merkle_root();
    let mut twin_snarked = snarked.copy();
    let mut twin = twin_snarked.make_child();
    let mut staged = snarked.make_child();
    let staged_uuid = staged.get_uuid();
    let mut ctx = LedgerCtx::default();
    for round in 0..ROUNDS {
        let end = (round + 1) * PER_ROUND;
        let start = end.saturating_sub(WINDOW * PER_ROUND);
        let mut block = staged.make_child();
        for index in round * PER_ROUND..end {
            block
                .set_at_index(
                    AccountIndex(index as u64),
                    Box::new(accounts[index].clone()),
                )
                .unwrap();
            twin.set_at_index(
                AccountIndex(index as u64),
                Box::new(accounts[index].clone()),
            )
            .unwrap();
        }
        // Emit the oldest round. Every staged account outside the fixed
        // pending window now equals the parent; the twin never reclaims it.
        if round >= WINDOW {
            let mut emitted = snarked.make_child();
            for index in start - PER_ROUND..start {
                let account = Box::new(accounts[index].clone());
                emitted
                    .set_at_index(AccountIndex(index as u64), account.clone())
                    .unwrap();
                twin_snarked
                    .set_at_index(AccountIndex(index as u64), account)
                    .unwrap();
            }
            emitted.merkle_root();
            emitted.commit();
            emitted.remove_and_reparent();
        }
        let root_hash = merkle_root(&mut snarked);
        assert_eq!(snarked.merkle_root(), twin_snarked.merkle_root());
        ctx.snarked_ledgers.clear();
        ctx.snarked_ledgers
            .insert(root_hash.clone(), snarked.clone());
        ctx.staged_ledgers.take();
        let hash = insert_staged(
            &mut ctx,
            block,
            pending_scan(accounts[start..end].iter().cloned()),
        );
        assert!(ctx.commit_root_staged_ledger(&hash, &root_hash));
        staged = ctx.staged_ledgers.get(&hash).unwrap().ledger();
        assert_eq!(staged.get_uuid(), staged_uuid);
        assert_eq!(staged.num_own_accounts(), end - start, "round {round}");
        assert!(staged.get_raw_inner_hashes().len() <= 2 * WINDOW * PER_ROUND * (LEDGER_DEPTH + 1));
        assert_eq!(twin.num_own_accounts(), end);
        assert_eq!(staged.merkle_root(), twin.merkle_root(), "round {round}");
    }
    assert_eq!(staged.num_own_accounts(), WINDOW * PER_ROUND);
    assert_eq!(twin.num_own_accounts(), ROUNDS * PER_ROUND);
}
