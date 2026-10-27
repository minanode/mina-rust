use super::*;

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/test/indexed_pool_tests.ml#L28-L28>
#[test]
fn empty_invariants() {
    let mut pool = IndexedPool::new(&constants());
    assert_pool_consistency(&pool);
    assert!(pool.remove_lowest_fee().unwrap().is_empty());
    assert!(pool.list_includable_transactions(100).is_empty());
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/test/indexed_pool_tests.ml#L30-L62>
#[test]
fn singleton_properties() {
    for (amount, fee) in [(0, 0), (400, 99), (400, 100), (400, 101), (500, 10)] {
        let mut pool = IndexedPool::new(&constants());
        let cmd = payment_until(0, 0, amount, fee, 10);
        let result = add(&mut pool, &cmd, 0, 500);
        if amount + fee > 500 {
            assert!(matches!(
                result,
                Err(CommandError::InsufficientFunds { .. })
            ));
        } else {
            assert!(result.unwrap().is_empty());
            assert_eq!(pool.size(), 1);
            assert_eq!(pool.list_includable_transactions(1), vec![cmd.clone()]);
            assert_eq!(pool.remove_lowest_fee().unwrap(), VecDeque::from([cmd]));
        }
        assert_eq!(pool.size(), 0);
        assert_pool_consistency(&pool);
    }
}

// Fixed, reproducible generator: interleave sender queues without changing
// within-sender nonce order. Amounts/fees stay inside each initial balance.
fn generated(
    seed: u64,
    senders: usize,
) -> (BTreeMap<AccountId, Account>, Vec<ValidCommandWithHash>) {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut accts = accounts(100_000);
    let mut queues: Vec<VecDeque<_>> = (0..senders)
        .map(|i| {
            let initial = rng.gen_range(0..1000);
            accts.get_mut(&id(i)).unwrap().nonce = Nonce::from_u32(initial);
            (0..rng.gen_range(6..12))
                .map(|n| payment(i, initial + n, rng.gen_range(1..500), rng.gen_range(1..100)))
                .collect()
        })
        .collect();
    let mut cmds = Vec::new();
    while queues.iter().any(|q| !q.is_empty()) {
        let i = rng.gen_range(0..senders);
        if let Some(c) = queues[i].pop_front() {
            cmds.push(c);
        }
    }
    (accts, cmds)
}

fn populated(accts: &BTreeMap<AccountId, Account>, cmds: &[ValidCommandWithHash]) -> IndexedPool {
    let mut pool = IndexedPool::new(&constants());
    for c in cmds {
        let a = &accts[&c.data.fee_payer()];
        assert!(add(&mut pool, c, a.nonce.as_u32(), a.balance.as_u64())
            .unwrap()
            .is_empty());
    }
    pool
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/test/indexed_pool_tests.ml#L64-L161>
#[test]
fn sequential_adds_all_valid() {
    for seed in 0..64 {
        let (accts, cmds) = generated(seed, 4);
        let pool = populated(&accts, &cmds);
        assert_eq!(hashes(pool.all_by_hash.values()), hashes(&cmds));
    }
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/test/indexed_pool_tests.ml#L163-L320>
#[test]
fn replacement() {
    // Replacement can occur at every position; rejection must be atomic.
    for initial in [0, 37] {
        for index in 0..4 {
            for amount in [1, 700, 1001] {
                let mut pool = IndexedPool::new(&constants());
                let cmds: Vec<_> = (0..4).map(|n| payment(0, initial + n, 200, 10)).collect();
                for c in &cmds {
                    add(&mut pool, c, initial, 1000).unwrap();
                }
                let c = payment(0, initial + index, amount, 100);
                let result = add(&mut pool, &c, initial, 1000);
                if u64::from(index) * 210 + amount + 100 > 1000 {
                    assert!(matches!(
                        result,
                        Err(CommandError::InsufficientFunds { .. })
                    ));
                    assert_eq!(hashes(pool.all_by_hash.values()), hashes(&cmds));
                } else {
                    let dropped = result.unwrap();
                    assert!(dropped.contains(&cmds[index as usize]));
                    // OCaml's go returns drop_head plus the tail *after*
                    // the first failed reinsertion (L1126-L1141). Preserve
                    // that upstream behavior; membership still drops all
                    // unaffordable dependents.
                    let kept_tail = (1000 - u64::from(index) * 210 - amount - 100) / 210;
                    let first_failed = index as usize + 1 + kept_tail as usize;
                    let expected = std::iter::once(&cmds[index as usize])
                        .chain(cmds.iter().skip(first_failed + 1));
                    assert_eq!(hashes(&dropped), hashes(expected));
                    assert!(pool.member(&c));
                }
            }
        }
    }
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/test/indexed_pool_tests.ml#L322-L361>
#[test]
fn remove_lowest_fee() {
    let cmds: Vec<_> = [40, 10, 30, 20]
        .into_iter()
        .enumerate()
        .map(|(i, fee)| payment(i, 0, 1, fee))
        .collect();
    let mut pool = populated(&accounts(1000), &cmds);
    assert_eq!(
        pool.remove_lowest_fee().unwrap(),
        VecDeque::from([cmds[1].clone()])
    );
    assert_eq!(
        hashes(pool.all_by_hash.values()),
        hashes([&cmds[0], &cmds[2], &cmds[3]])
    );
    assert_pool_consistency(&pool);
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/test/indexed_pool_tests.ml#L373-L390>
#[test]
fn pick_highest_fee_for_application() {
    let cmds: Vec<_> = [40, 10, 30, 20]
        .into_iter()
        .enumerate()
        .map(|(i, fee)| payment(i, 0, 1, fee))
        .collect();
    let mut pool = populated(&accounts(1000), &cmds);
    let expected = vec![
        cmds[0].clone(),
        cmds[2].clone(),
        cmds[3].clone(),
        cmds[1].clone(),
    ];
    assert_eq!(pool.list_includable_transactions(4), expected);
    assert_eq!(pool.transactions(4), expected);
    // OCaml transactions operates on a persistent copy; reading candidates
    // must leave the original pool valid and unchanged.
    assert_pool_consistency(&pool);
    assert_eq!(pool.size(), 4);
    assert_eq!(pool.list_includable_transactions(4), expected);
}

fn revalidate_after(
    pool: &mut IndexedPool,
    accts: &mut BTreeMap<AccountId, Account>,
    cmd: &ValidCommandWithHash,
    expected: &[ValidCommandWithHash],
) {
    apply_to_accounts(accts, std::slice::from_ref(cmd));
    let referenced: BTreeSet<_> = cmd
        .data
        .forget_check()
        .accounts_referenced()
        .into_iter()
        .collect();
    let dropped = pool
        .revalidate(Slot::zero(), RevalidateKind::Subset(&referenced), |id| {
            accts.get(id).cloned()
        })
        .unwrap();
    assert_eq!(hashes(&dropped), hashes(expected));
    assert_pool_consistency(pool);
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/test/indexed_pool_tests.ml#L587-L618>
#[test]
fn support_for_zkapp_command_commands() {
    let mut accts = accounts(100 * MINA);
    let cmds: Vec<_> = (0..4).map(|n| zkapp(0, 1, n, MINA, false, false)).collect();
    let mut pool = populated(&accts, &cmds);
    revalidate_after(&mut pool, &mut accts, &cmds[0], &cmds[..1]);
    revalidate_after(&mut pool, &mut accts, &cmds[1], &cmds[1..2]);
    let double = zkapp(0, 1, 2, MINA, true, false);
    revalidate_after(&mut pool, &mut accts, &double, &cmds[2..]);
    assert_eq!(pool.size(), 0);
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/test/indexed_pool_tests.ml#L620-L651>
#[test]
fn nonce_increment_side_effects() {
    let mut accts = accounts(100 * MINA);
    let cmds = [
        zkapp(0, 1, 0, MINA, false, true),
        zkapp(1, 0, 0, MINA, false, false),
        zkapp(1, 0, 1, MINA, false, false),
    ];
    let mut pool = populated(&accts, &cmds);
    revalidate_after(&mut pool, &mut accts, &cmds[0], &cmds[..2]);
    assert_eq!(hashes(pool.all_by_hash.values()), hashes(&cmds[2..]));
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/test/indexed_pool_tests.ml#L653-L679>
#[test]
fn nonce_invariant_violation() {
    let mut accts = accounts(100 * MINA);
    let cmds = [
        zkapp(0, 1, 0, MINA, false, false),
        zkapp(0, 1, 1, MINA, false, false),
    ];
    let mut pool = populated(&accts, &cmds);
    apply_to_accounts(&mut accts, &cmds[..1]);
    revalidate_after(&mut pool, &mut accts, &cmds[1], &cmds);
}

fn nonce_order(senders: usize) {
    for seed in 0..64 {
        let (accts, cmds) = generated(seed, senders);
        let mut pool = populated(&accts, &cmds);
        for _ in 0..5 {
            pool.remove_lowest_fee().unwrap();
            assert_pool_consistency(&pool);
        }
        let mut expected: BTreeMap<_, _> = accts.iter().map(|(id, a)| (id, a.nonce)).collect();
        for c in pool.list_includable_transactions(1000) {
            let cmd = c.data.forget_check();
            let nonce = expected.get_mut(&cmd.fee_payer()).unwrap();
            assert_eq!(cmd.applicable_at_nonce(), *nonce);
            *nonce = nonce.succ();
        }
        assert_pool_consistency(&pool);
    }
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/test/indexed_pool_tests.ml#L681-L702>
#[test]
fn transactions_from_single_sender_ordered_by_nonce() {
    nonce_order(1);
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/test/indexed_pool_tests.ml#L734-L744>
#[test]
fn transactions_from_many_senders_no_nonce_gaps() {
    nonce_order(4);
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/test/indexed_pool_tests.ml#L746-L763>
#[test]
fn revalidation_drops_nothing_unless_ledger_changed() {
    for seed in 0..64 {
        let (accts, cmds) = generated(seed, 4);
        let mut pool = populated(&accts, &cmds);
        let before = pool.list_includable_transactions(1000);
        let dropped = pool
            .revalidate(Slot::zero(), RevalidateKind::EntirePool, |id| {
                accts.get(id).cloned()
            })
            .unwrap();
        assert!(dropped.is_empty());
        assert_eq!(pool.list_includable_transactions(1000), before);
        assert_pool_consistency(&pool);
    }
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/test/indexed_pool_tests.ml#L787-L812>
#[test]
fn application_invalidates_applied_transactions() {
    for seed in 0..64 {
        let (mut accts, cmds) = generated(seed, 4);
        let mut pool = populated(&accts, &cmds);
        let count = seed as usize * 7 % (cmds.len() + 1);
        apply_to_accounts(&mut accts, &cmds[..count]);
        let dropped = pool
            .revalidate(Slot::zero(), RevalidateKind::EntirePool, |id| {
                accts.get(id).cloned()
            })
            .unwrap();
        assert_eq!(hashes(&dropped), hashes(&cmds[..count]));
        assert_pool_consistency(&pool);
    }
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/test/indexed_pool_tests.ml#L821-L886>
#[test]
fn transaction_replacement() {
    for n in 0..6 {
        let cmds: Vec<_> = (0..6).map(|n| payment(0, n, 100, 10)).collect();
        let mut pool = populated(&accounts(660), &cmds);
        let replacement = payment(0, n, 100, 11);
        assert_eq!(
            add(&mut pool, &replacement, 0, 661).unwrap(),
            VecDeque::from([cmds[n as usize].clone()])
        );
        let mut expected = cmds;
        expected[n as usize] = replacement;
        assert_eq!(pool.list_includable_transactions(100), expected);
    }
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/test/indexed_pool_tests.ml#L888-L953>
#[test]
fn transaction_replacement_insufficient_balance() {
    let cmds: Vec<_> = (0..3)
        .map(|n| payment(0, n, 10 * MINA, 300_000_000))
        .collect();
    let mut pool = populated(&accounts(31 * MINA), &cmds);
    let replacement = payment(0, 1, 10 * MINA, 700_000_000);
    let dropped = add(&mut pool, &replacement, 0, 31 * MINA).unwrap();
    assert_eq!(
        pool.list_includable_transactions(100),
        vec![cmds[0].clone(), replacement]
    );
    // Matches OCaml: the first failed dependent is absent from the report.
    assert_eq!(hashes(&dropped), hashes(&cmds[1..2]));
}
