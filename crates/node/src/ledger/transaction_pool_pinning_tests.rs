use super::*;
use ledger::scan_state::currency::Nonce;

fn root(nonce: u32) -> Mask {
    let mut root = Mask::new_root(Database::create(LEDGER_DEPTH as u8));
    let mut account = Account::empty();
    account.nonce = Nonce::from_u32(nonce);
    root.get_or_create_account(account.id(), account).unwrap();
    root
}

fn child(parent: &Mask, nonce: u32) -> Mask {
    let mut child = parent.make_child();
    let mut account = Account::empty();
    account.nonce = Nonce::from_u32(nonce);
    let addr = child.location_of_account(&account.id()).unwrap();
    child.set(addr, Box::new(account));
    child
}

fn staged(ctx: &mut LedgerCtx, mask: Mask) -> Arc<MinaBaseStagedLedgerHashStableV1> {
    let mut ledger = StagedLedger::create_exn(constraint_constants().clone(), mask).unwrap();
    let hash = Arc::new(MinaBaseStagedLedgerHashStableV1::from(&ledger.hash()));
    ctx.staged_ledgers.insert(hash.clone(), ledger);
    hash
}

#[test]
fn transaction_pool_pin_protects_snarked_ancestors_until_read() {
    let mut ctx = LedgerCtx::default();
    let mut base = root(0);
    let mut old = child(&base, 1);
    let mut new = child(&old, 2);
    let old_hash = merkle_root(&mut old);
    let new_hash = merkle_root(&mut new);
    ctx.snarked_ledgers.insert(merkle_root(&mut base), base);
    ctx.snarked_ledgers.insert(old_hash.clone(), old.clone());
    ctx.snarked_ledgers.insert(new_hash.clone(), new);
    ctx.pin_transaction_pool_ledger(1, &old_hash);
    let mut keep = LedgersToKeep::new();
    keep.add_snarked(new_hash.clone());
    ctx.transaction_pool_keep_pinned(&mut keep);
    ctx.drop_snarked_ledgers(&keep);
    ctx.compact_snarked_ledgers(&keep);
    let id = Account::empty().id();
    let accounts = ctx
        .transaction_pool_accounts(1, &old_hash, vec![id.clone()], true)
        .unwrap();
    assert_eq!(accounts[&id].nonce, Nonce::from_u32(1));
    let mut keep = LedgersToKeep::new();
    keep.add_snarked(new_hash.clone());
    ctx.drop_snarked_ledgers(&keep);
    ctx.compact_snarked_ledgers(&keep);
    let accounts = ctx
        .transaction_pool_accounts(2, &new_hash, vec![id.clone()], false)
        .unwrap();
    assert_eq!(accounts[&id].nonce, Nonce::from_u32(2));
    assert!(ctx.transaction_pool_ledgers.is_empty());
}

#[test]
fn transaction_pool_pin_defers_staged_fold_and_resumes_over_a_new_snarked_base() {
    let _ = mina_core::network::NetworkConfig::init("mainnet");
    let mut ctx = LedgerCtx::default();
    let mut base = root(0);
    let base_hash = merkle_root(&mut base);
    ctx.snarked_ledgers.insert(base_hash.clone(), base.clone());
    let old_root = child(&base, 1);
    let mut old_tip = child(&old_root, 2);
    let tip_hash = merkle_root(&mut old_tip);
    let new_root = child(&old_tip, 3);
    staged(&mut ctx, old_root);
    staged(&mut ctx, old_tip.clone());
    let new_staged_hash = staged(&mut ctx, new_root);
    ctx.pin_transaction_pool_ledger(1, &tip_hash);
    ctx.commit_root_staged_ledger(&new_staged_hash, &base_hash)
        .unwrap();
    assert!(
        old_tip.is_attached(),
        "a live pin must not be detached by staged compaction"
    );
    let id = Account::empty().id();
    let accounts = ctx
        .transaction_pool_accounts(1, &tip_hash, vec![id.clone()], true)
        .unwrap();
    assert_eq!(accounts[&id].nonce, Nonce::from_u32(2));

    // Simulate a newer snarked root detached from the old epoch base. The old
    // base leaves the kept set before the next staged fold, as during commit.
    let mut new_snarked = root(2);
    let new_snarked_hash = merkle_root(&mut new_snarked);
    let new_uuid = new_snarked.get_uuid();
    ctx.snarked_ledgers
        .insert(new_snarked_hash.clone(), new_snarked);
    let mut keep = LedgersToKeep::new();
    keep.add_snarked(new_snarked_hash.clone());
    ctx.drop_snarked_ledgers(&keep);
    ctx.compact_snarked_ledgers(&keep);
    ctx.commit_root_staged_ledger(&new_staged_hash, &new_snarked_hash)
        .unwrap();
    let current = ctx.staged_ledgers.get(&new_staged_hash).unwrap().ledger();
    assert_eq!(current.get_parent().unwrap().get_uuid(), new_uuid);
    assert_eq!(
        current
            .get(current.location_of_account(&id).unwrap())
            .unwrap()
            .nonce,
        Nonce::from_u32(3)
    );
    assert_eq!(
        base.get(base.location_of_account(&id).unwrap())
            .unwrap()
            .nonce,
        Nonce::from_u32(0)
    );
    assert!(!old_tip.is_attached());
    assert!(ctx.transaction_pool_ledgers.is_empty());
    assert!(ctx.transaction_pool_deferred_bases.is_empty());
}
