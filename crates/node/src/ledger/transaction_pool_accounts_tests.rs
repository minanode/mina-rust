use super::*;
use ledger::{BaseLedger, Database};

fn ledger() -> (LedgerCtx, LedgerHash) {
    let mut mask = Mask::new_root(Database::create(super::super::LEDGER_DEPTH as u8));
    let hash = LedgerHash::from_fp(mask.merkle_root());
    let mut ctx = LedgerCtx::default();
    ctx.insert_genesis_ledger(mask);
    (ctx, hash)
}

#[test]
fn transaction_pool_snapshot_survives_pruning_and_is_released_after_read() {
    let (mut ctx, hash) = ledger();
    ctx.pin_transaction_pool_ledger(7, &hash);
    assert_eq!(ctx.memory_debug()["transaction_pool_ledger_pins"], 1);
    ctx.invalidate_for_resync();
    assert_eq!(
        ctx.transaction_pool_accounts(8, &hash, vec![], false),
        Err("Ledger unavailable".into())
    );
    assert!(ctx
        .transaction_pool_accounts(7, &hash, vec![], true)
        .unwrap()
        .is_empty());
    assert_eq!(ctx.memory_debug()["transaction_pool_ledger_pins"], 0);
}

#[test]
fn transaction_pool_queued_read_is_cancelled_before_account_lookup() {
    let (mut ctx, hash) = ledger();
    let (caller, _) = LedgerCaller::queued();
    let request = LedgerRequest::TransactionPoolAccounts {
        id: 7,
        ledger_hash: hash,
        account_ids: vec![],
        pinned: false,
        generation: 0,
        current_generation: Arc::new(AtomicU64::new(1)),
    };
    assert!(
        matches!(request.handle(&mut ctx, &caller), LedgerResponse::TransactionPoolAccounts(7, Err(e)) if e == "transaction account read cancelled")
    );
}

#[test]
fn transaction_pool_async_read_returns_an_event_including_failure() {
    let (mut ctx, hash) = ledger();
    let (sender, mut receiver) = mpsc::unbounded_channel();
    ctx.set_event_sender(sender);
    let manager = LedgerManager::spawn(ctx);
    manager.transaction_pool_accounts(1, hash, vec![], 0, false);
    manager.transaction_pool_accounts(2, LedgerHash::zero(), vec![], 0, false);
    let first = receiver.blocking_recv().unwrap();
    let second = receiver.blocking_recv().unwrap();
    assert!(matches!(
        first,
        crate::event_source::Event::Ledger(super::super::LedgerEvent::TransactionPoolAccounts {
            id: 1,
            result: Ok(_)
        })
    ));
    assert!(
        matches!(second, crate::event_source::Event::Ledger(super::super::LedgerEvent::TransactionPoolAccounts { id: 2, result: Err(e) }) if e == "Ledger unavailable")
    );
}

#[test]
fn transaction_pool_async_worker_loss_keeps_the_fatal_exit_policy() {
    const CHILD: &str = "MINA_TXPOOL_WORKER_FAILURE_TEST";
    if std::env::var_os(CHILD).is_some() {
        let manager = LedgerManager::spawn(LedgerCtx::default());
        manager.call(LedgerRequest::TestPanic);
        std::thread::sleep(std::time::Duration::from_secs(5));
        panic!("a lost ledger worker left async requests hanging");
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "ledger::ledger_manager::transaction_pool_accounts_tests::transaction_pool_async_worker_loss_keeps_the_fatal_exit_policy", "--nocapture"])
        .env(CHILD, "1").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("ledger-manager worker stopped"));
}

#[test]
fn transaction_pool_failed_pin_can_be_reacquired_after_ledger_load() {
    let (loaded, hash) = ledger();
    let mut ctx = LedgerCtx::default();
    ctx.pin_transaction_pool_ledger(1, &hash);
    assert_eq!(
        ctx.transaction_pool_accounts(1, &hash, vec![], true),
        Err("Ledger unavailable".into())
    );
    assert_eq!(ctx.memory_debug()["transaction_pool_ledger_pins"], 0);
    let (mask, _) = loaded.mask(&hash).unwrap();
    ctx.insert_genesis_ledger(mask);
    ctx.pin_transaction_pool_ledger(2, &hash);
    ctx.invalidate_for_resync();
    assert!(ctx
        .transaction_pool_accounts(2, &hash, vec![], true)
        .unwrap()
        .is_empty());
    assert_eq!(ctx.memory_debug()["transaction_pool_ledger_pins"], 0);
}
