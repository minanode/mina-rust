use super::*;

fn linear(zk: bool) {
    let mut h = Harness::new();
    let cmds = independent(zk);
    h.add(&cmds, false);
    // As in OCaml, signed payments can invalidate queued zkApps by nonce.
    let payments = independent(false);
    h.commit(&payments[..1]);
    h.assert_txs(&cmds[1..]);
    h.commit(&payments[1..3]);
    h.assert_txs(&cmds[3..]);
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2371-L2388>
#[test]
fn transactions_are_removed_in_linear_case_user_cmds() {
    linear(false);
}
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2390-L2394>
#[test]
fn transactions_are_removed_in_linear_case_zkapps() {
    linear(true);
}

fn fork_changes(zk: bool) {
    let mut h = Harness::new();
    let cmds = independent(zk);
    let initial = h.accounts.clone();
    h.add(&[cmds[..1].to_vec(), cmds[2..].to_vec()].concat(), false);
    apply_to_accounts(&mut h.accounts, &cmds[..1]);
    h.diff(&cmds[..1], &cmds[1..2]);
    h.assert_txs(&cmds[1..]);
    // Also exercise multiple same-sender commands, which must backtrack in
    // reverse order. Independent commands alone cannot detect E1.
    h = Harness::new();
    let seq: Vec<_> = (0..4)
        .map(|n| {
            if zk {
                zkapp(0, 15, n, FEE, false, false)
            } else {
                payment(0, n, MINA, FEE)
            }
        })
        .collect();
    h.add(&seq, true);
    h.commit(&seq[..3]);
    h.accounts = initial;
    h.diff(&[], &seq[..3]);
    h.assert_txs(&seq);
    h.assert_rebroadcastable(&seq);
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2396-L2409>
#[test]
fn transactions_are_removed_and_added_back_in_fork_changes_user_cmds() {
    fork_changes(false);
}
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2411-L2416>
#[test]
fn transactions_are_removed_and_added_back_in_fork_changes_zkapps() {
    fork_changes(true);
}

/// Backtracking evicts local nonces 2, 1, 0 while higher-fee remote commands
/// occupy all slots. Once those commands commit, OCaml retries locals 0, 1, 2.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L630-L633>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L727-L790>
#[test]
fn locally_evicted_backtrack_retries_follow_ocaml_nonce_order() {
    let mut h = Harness::new();
    h.pool.config.pool_max_size = 3;
    let initial = h.accounts.clone();
    let local: Vec<_> = (0..3).map(|n| payment(0, n, MINA, FEE)).collect();
    h.add(&local, true);
    h.commit(&local);
    h.assert_txs(&[]);
    h.assert_rebroadcastable(&[]);

    let remote: Vec<_> = (1..4).map(|i| payment(i, 0, MINA, 2 * FEE)).collect();
    h.add(&remote, false);
    h.assert_txs(&remote);

    // Restore the common ancestor, then model the winning chain's ledger.
    h.accounts = initial;
    apply_to_accounts(&mut h.accounts, &remote);
    h.diff(&remote, &local);

    assert_eq!(h.pool.list_includable_transactions(10), local);
    h.assert_txs(&local);
    h.assert_rebroadcastable(&local);
}

fn invalid(zk: bool) {
    let mut h = Harness::new();
    let cmds = independent(zk);
    h.commit(&cmds[..2]);
    let (accepted, rejected) = h.submit(&cmds, false);
    assert_eq!(hashes(&accepted), hashes(&cmds[2..]));
    assert_eq!(rejected.len(), 2);
    assert!(rejected
        .iter()
        .all(|(_, e)| matches!(e, diff::Error::InvalidNonce)));
    h.assert_txs(&cmds[2..]);
}
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2418-L2430>
#[test]
fn invalid_transactions_are_not_accepted_user_cmds() {
    invalid(false);
}
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2432-L2437>
#[test]
fn invalid_transactions_are_not_accepted_zkapps() {
    invalid(true);
}

fn now_invalid(zk: bool) {
    let mut h = Harness::new();
    let cmd = if zk {
        zkapp(0, 15, 0, FEE, false, false)
    } else {
        payment(0, 0, 99_999_999_999, FEE)
    };
    h.add(&[cmd], true);
    // Same fee payer/nonce, different winning command.
    let winning = payment(0, 0, 999_000_000_000, FEE);
    apply_to_accounts(&mut h.accounts, std::slice::from_ref(&winning));
    h.diff(&[winning], &[]);
    h.assert_txs(&[]);
    h.assert_rebroadcastable(&[]);
}
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2446-L2467>
#[test]
fn now_invalid_transactions_removed_on_fork_changes_user_cmds() {
    now_invalid(false);
}
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2469-L2477>
#[test]
fn now_invalid_transactions_removed_on_fork_changes_zkapps() {
    now_invalid(true);
}

fn expired_not_accepted(zk: bool) {
    let mut h = Harness::new();
    let cmds = independent(zk);
    h.slot = 11;
    let valid = payment_until(1, 1, MINA, FEE, 20);
    let expired = payment_until(0, 1, MINA, FEE, 10);
    let dependent = payment(0, 2, MINA, FEE);
    let mut valid_cmds = cmds;
    valid_cmds.push(valid);
    let all = [valid_cmds.clone(), vec![expired, dependent]].concat();
    let (accepted, rejected) = h.submit(&all, false);
    assert_eq!(hashes(&accepted), hashes(&valid_cmds));
    assert!(matches!(rejected[0].1, diff::Error::Expired));
    assert!(matches!(rejected[1].1, diff::Error::InvalidNonce));
    h.assert_txs(&valid_cmds);
}
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2479-L2526>
#[test]
fn expired_transactions_are_not_accepted_user_cmds() {
    expired_not_accepted(false);
}
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2528-L2532>
#[test]
fn expired_transactions_are_not_accepted_zkapps() {
    expired_not_accepted(true);
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2534-L2614>
#[test]
fn expired_transactions_removed_on_best_tip_changes_user_commands() {
    let mut h = Harness::new();
    let few = independent(false)[..3].to_vec();
    let first = payment_until(0, 1, 10 * MINA, FEE, 3);
    let second = payment_until(0, 2, 10 * MINA, FEE, 7);
    h.add(
        &[few.clone(), vec![first.clone(), second.clone()]].concat(),
        false,
    );
    let initial = h.accounts.clone();
    h.commit(&[few[0].clone(), first.clone()]);
    h.assert_txs(&[few[1..].to_vec(), vec![second.clone()]].concat());
    let expired = payment_until(9, 0, MINA, FEE, 0);
    let unexpired = payment_until(8, 0, MINA, FEE, 7);
    h.slot = 3;
    h.accounts = initial;
    apply_to_accounts(&mut h.accounts, &few[..1]);
    h.diff(
        &few[..1],
        &[few[0].clone(), first.clone(), expired, unexpired.clone()],
    );
    h.assert_txs(&[few[1..].to_vec(), vec![first, second, unexpired]].concat());
    h.slot = 8;
    h.diff(&[], &[]);
    h.assert_txs(&few[1..]);
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2616-L2664>
#[test]
fn expired_transactions_removed_on_best_tip_changes_zkapps() {
    let mut h = Harness::new();
    let few = independent(false)[..3].to_vec();
    let expiring = |sender, until| {
        alter_zkapp(&zkapp(sender, 15, 1, FEE, false, false), sender, |c| {
            c.fee_payer.body.valid_until = Some(Slot::from_u32(until));
            c.account_updates = c.account_updates.map_to(|u| {
                let mut u = u.clone();
                u.body.preconditions.valid_while = zk::Numeric::Check(zk::ClosedInterval {
                    lower: Slot::zero(),
                    upper: Slot::from_u32(until),
                });
                u
            });
        })
    };
    let a = expiring(0, 3);
    let b = expiring(2, 7);
    h.add(&[few.clone(), vec![a, b.clone()]].concat(), false);
    h.slot = 4;
    h.diff(&[], &[]);
    h.assert_txs(&[few.clone(), vec![b]].concat());
    h.slot = 9;
    h.diff(&[], &[]);
    h.assert_txs(&few);
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2666-L2688>
#[test]
fn now_invalid_transactions_removed_when_frontier_recreated() {
    let mut h = Harness::new();
    let cmds = independent(false);
    h.add(&cmds, true);
    h.accounts.get_mut(&id(0)).unwrap().nonce = Nonce::from_u32(5);
    h.accounts.get_mut(&id(1)).unwrap().balance = Balance::zero();
    let a = h.accounts.get_mut(&id(2)).unwrap();
    a.balance = Balance::zero();
    a.nonce = Nonce::from_u32(1);
    let dropped = h.pool.on_new_best_tip(Slot::zero(), &h.accounts).unwrap();
    assert_eq!(hashes(&dropped), hashes(&cmds[..3]));
    h.assert_txs(&cmds[3..]);
    h.assert_rebroadcastable(&cmds[3..]);
}

fn with_vk(sender: usize, nonce: u32) -> ValidCommandWithHash {
    alter_zkapp(&zkapp(sender, 15, nonce, FEE, false, false), sender, |c| {
        c.account_updates = c.account_updates.map_to(|u| {
            let mut u = u.clone();
            if u.body.public_key == KEYS[sender].public.into_compressed() {
                u.body.update.verification_key = zk::SetOrKeep::Set(VerificationKeyWire::dummy());
            }
            u
        });
    })
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2690-L2723>
#[test]
fn vk_refcounts_decremented_when_frontier_recreated() {
    let mut h = Harness::new();
    let cmds: Vec<_> = (0..7).map(|i| with_vk(i, 0)).collect();
    h.add(&cmds, false);
    assert_eq!(h.pool.verification_key_table.verification_keys.len(), 1);
    // Dropping one reference must preserve the other six account mappings.
    h.accounts.get_mut(&id(0)).unwrap().nonce = Nonce::from_u32(5);
    h.pool.on_new_best_tip(Slot::zero(), &h.accounts).unwrap();
    let table = &h.pool.verification_key_table;
    let vk_hash = VerificationKeyWire::dummy().hash();
    assert_eq!(table.verification_keys[&vk_hash].0, 6);
    assert_eq!(table.vk_to_account_ids[&vk_hash].len(), 6);
    assert_eq!(table.account_id_to_vks.len(), 6);
    h.assert_txs(&cmds[1..]);
    for i in 1..7 {
        h.accounts.get_mut(&id(i)).unwrap().nonce = Nonce::from_u32(5);
    }
    h.pool.on_new_best_tip(Slot::zero(), &h.accounts).unwrap();
    h.assert_txs(&[]);
    assert!(h.pool.verification_key_table.verification_keys.is_empty());
    assert!(h.pool.verification_key_table.account_id_to_vks.is_empty());
    assert!(h.pool.verification_key_table.vk_to_account_ids.is_empty());
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2725-L2807>
#[test]
fn transaction_replacement_works() {
    let mut h = Harness::new();
    let cmds: Vec<_> = (0..4)
        .flat_map(|i| {
            [20, 12, 500]
                .into_iter()
                .enumerate()
                .map(move |(n, amount)| payment(i, n as u32, amount * MINA, FEE))
        })
        .collect();
    h.add(&cmds, false);
    let replacements = [
        payment(0, 0, 440 * MINA, FEE + 1),
        payment(1, 0, 788 * MINA, FEE),
        payment(2, 1, 721 * MINA, FEE + 1),
        payment(3, 1, 927 * MINA, BALANCE - 927 * MINA),
    ];
    let (accepted, rejected) = h.submit(&replacements, false);
    assert_eq!(
        hashes(&accepted),
        hashes([&replacements[0], &replacements[2]])
    );
    assert_eq!(rejected.len(), 2);
    assert!(matches!(rejected[0].1, diff::Error::InsufficientReplaceFee));
    assert!(matches!(rejected[1].1, diff::Error::InsufficientFunds));
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2809-L2832>
#[test]
fn drops_queued_transactions_when_committed_one_leaves_insufficient_funds() {
    let mut h = Harness::new();
    let cmds: Vec<_> = [20, 77, 891]
        .into_iter()
        .enumerate()
        .map(|(n, a)| payment(0, n as u32, a * MINA, FEE))
        .collect();
    h.add(&cmds, true);
    let a = h.accounts.get_mut(&id(0)).unwrap();
    a.balance = Balance::from_u64(970 * MINA);
    a.nonce = Nonce::from_u32(1);
    h.diff(&[payment(0, 0, 25 * MINA, FEE)], &[]);
    h.assert_txs(&cmds[1..2]);
    h.assert_rebroadcastable(&cmds[1..2]);
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2834-L2868>
#[test]
fn max_size_is_maintained() {
    for seed in 0..64 {
        let mut rng = StdRng::seed_from_u64(seed);
        let mut h = Harness::new();
        let mut next = [0; 7];
        let cmds: Vec<_> = (0..50)
            .map(|_| {
                let i = rng.gen_range(0..7);
                let n = next[i];
                next[i] += 1;
                payment(i, n, MINA, FEE + rng.gen_range(0..1_000_000))
            })
            .collect();
        h.add(&cmds[..25], false);
        assert_eq!(h.pool.size(), 25);
        h.submit(&cmds[25..], false);
        assert!(h.pool.size() <= 25);
    }
}

fn rebroadcastable(zk: bool) {
    let mut h = Harness::new();
    let cmds = independent(zk);
    let initial = h.accounts.clone();
    h.assert_rebroadcastable(&[]);
    h.add(&cmds[..2], true);
    h.add(&cmds[2..5], false);
    h.assert_rebroadcastable(&cmds[..2]);
    h.add(&cmds[5..], true);
    h.commit(&cmds[..1]);
    let checkpoint = h.accounts.clone();
    h.commit(&cmds[1..5]);
    h.assert_txs(&cmds[5..]);
    h.assert_rebroadcastable(&cmds[5..]);
    h.accounts = checkpoint;
    h.diff(&cmds[..1], &cmds[..5]);
    h.assert_txs(&cmds[1..]);
    h.assert_rebroadcastable(&[cmds[1..2].to_vec(), cmds[5..].to_vec()].concat());
    h.commit(&cmds[1..5]);
    h.assert_rebroadcastable(&cmds[5..]);
    h.accounts = initial;
    h.diff(&[], &cmds[..5]);
    h.assert_txs(&cmds);
    h.assert_rebroadcastable(&[cmds[..2].to_vec(), cmds[5..].to_vec()].concat());
    assert!(h.pool.get_rebroadcastable(|_| true).is_empty());
    h.assert_rebroadcastable(&[]);
    h.assert_txs(&cmds);
}
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2870-L2935>
#[test]
fn rebroadcastable_transaction_behavior_user_cmds() {
    rebroadcastable(false);
}
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2937-L2941>
#[test]
fn rebroadcastable_transaction_behavior_zkapps() {
    rebroadcastable(true);
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L2943-L2962>
#[test]
fn apply_user_cmds_and_zkapps() {
    let mut h = Harness::new();
    let cmds = [
        independent(true)[..3].to_vec(),
        independent(false)[3..].to_vec(),
    ]
    .concat();
    h.add(&cmds, false);
    h.assert_txs(&cmds);
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L3027-L3046>
#[test]
fn zkapp_same_nonce_replaces_previously_submitted_zkapp() {
    let mut h = Harness::new();
    let old = zkapp(0, 15, 0, 10 * MINA, false, false);
    let new = alter_zkapp(&old, 0, |c| {
        c.fee_payer.body.fee = Fee::from_u64(20 * MINA);
        c.account_updates = CallForest::empty();
    });
    let (accepted, rejected) = h.submit(&[old, new.clone()], false);
    assert_eq!(accepted, vec![new.clone()]);
    assert!(rejected.is_empty());
    h.assert_txs(&[new]);
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L3048-L3172>
#[test]
fn commands_rejected_if_fee_payer_permissions_not_handled() {
    for zk in [false, true] {
        for field in 0..3 {
            for (permission, allowed) in [
                (AuthRequired::Signature, true),
                (AuthRequired::Either, true),
                (AuthRequired::None, true),
                (AuthRequired::Impossible, false),
                (AuthRequired::Proof, false),
            ] {
                let mut h = Harness::new();
                let a = h.accounts.get_mut(&id(0)).unwrap();
                match field {
                    0 => a.permissions.send = permission,
                    1 => a.permissions.increment_nonce = permission,
                    _ => a.permissions.access = permission,
                }
                let cmd = if zk {
                    zkapp(0, 15, 0, FEE, false, false)
                } else {
                    payment(0, 0, MINA, FEE)
                };
                let (accepted, rejected) = h.submit(&[cmd.clone()], false);
                if allowed {
                    assert_eq!(accepted, vec![cmd]);
                    assert!(rejected.is_empty());
                } else {
                    assert!(accepted.is_empty());
                    assert!(matches!(
                        rejected[0].1,
                        diff::Error::FeePayerNotPermittedToSend
                    ));
                }
            }
        }
    }
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L3174-L3207>
#[test]
#[ignore = "P6d2 harness gap: needs a proof-authorized zkApp fixture signed for another network id"]
fn account_update_with_different_network_id_using_proof_is_rejected() {
    panic!("Requires an OCaml proof fixture; a signature-only negative case does not test proof network separation");
}

fn slot_tx_end(slot: u32, accepted: bool) {
    let mut h = Harness::new();
    h.pool = TransactionPool::new(
        Config {
            trust_system: (),
            pool_max_size: 25,
            slot_tx_end: Some(Slot::from_u32(10)),
        },
        &constants(),
    );
    h.slot = slot;
    let cmds = independent(false);
    let (actual, rejected) = h.submit(&cmds, false);
    if accepted {
        assert_eq!(hashes(&actual), hashes(&cmds));
        assert!(rejected.is_empty());
    } else {
        assert!(actual.is_empty());
        assert_eq!(rejected.len(), cmds.len());
        assert!(rejected
            .iter()
            .all(|(_, e)| matches!(e, diff::Error::AfterSlotTxEnd)));
    }
}
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L3209-L3221>
#[test]
fn transactions_added_before_slot_tx_end_are_accepted() {
    slot_tx_end(9, true);
}
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L3223-L3232>
#[test]
fn transactions_added_at_slot_tx_end_are_rejected() {
    slot_tx_end(10, false);
}
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L3234-L3249>
#[test]
fn transactions_added_after_slot_tx_end_are_rejected() {
    slot_tx_end(11, false);
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L3812-L4008>
#[test]
fn handle_transition_frontier_diff_permission_send_tx_updated() {
    for seed in 0..64 {
        let mut rng = StdRng::seed_from_u64(seed);
        let mut h = Harness::new();
        let prefix: Vec<_> = (0..4).map(|i| payment(i, 0, MINA, FEE)).collect();
        let minor: Vec<_> = (0..4)
            .flat_map(|i| (1..4).map(move |n| payment(i, n, 10 * MINA, FEE)))
            .collect();
        let permission = alter_zkapp(&zkapp(0, 15, 1, FEE, false, false), 0, |c| {
            c.account_updates = c.account_updates.map_to(|u| {
                let mut u = u.clone();
                if u.body.public_key == KEYS[0].public.into_compressed() {
                    let mut p = crate::account::Permissions::user_default();
                    if seed % 2 == 0 {
                        p.send = AuthRequired::Impossible;
                    } else {
                        p.increment_nonce = AuthRequired::Impossible;
                    }
                    u.body.update.permissions = zk::SetOrKeep::Set(p);
                }
                u
            });
        });
        let major = vec![
            permission,
            payment(1, 1, MINA, FEE),
            payment(2, 1, 970 * MINA, FEE),
        ];
        apply_to_accounts(&mut h.accounts, &prefix);
        apply_to_accounts(&mut h.accounts, &major);
        // Vary available capacity to retain zero, one, or two tail commands.
        let keep = rng.gen_range(0..=2usize);
        h.accounts.get_mut(&id(2)).unwrap().balance =
            Balance::from_u64(keep as u64 * (10 * MINA + FEE));
        h.diff(
            &[prefix.clone(), major].concat(),
            &[prefix, minor.clone()].concat(),
        );
        let expected: Vec<_> = minor
            .into_iter()
            .filter(|c| {
                let who = c.data.fee_payer();
                let n = c.data.forget_check().applicable_at_nonce().as_u32();
                who == id(1) && n >= 2
                    || who == id(2) && n >= 2 && n < 2 + keep as u32
                    || who == id(3)
            })
            .collect();
        h.assert_txs(&expected);
    }
}

/// Extension of OCaml's rebroadcast and max-size cases: dropped locals must
/// leave the rebroadcast table, both for replacement and capacity eviction.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L1422-L1427>
#[test]
fn locally_generated_evictions_are_not_rebroadcastable() {
    let mut h = Harness::new();
    h.pool.config.pool_max_size = 1;
    let first = payment(0, 0, MINA, FEE);
    let replacement = payment(0, 0, MINA, FEE + 1);
    h.add(&[first], true);
    h.add(&[replacement.clone()], true);
    h.assert_rebroadcastable(&[replacement]);
    let higher = payment(1, 0, MINA, FEE + 2);
    h.add(&[higher.clone()], false);
    h.assert_txs(&[higher]);
    h.assert_rebroadcastable(&[]);
}

/// Verify the generated mainnet authorizations independently of pool admission.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L1982-L2013>
#[test]
fn generated_commands_have_valid_mainnet_signatures() {
    // NetworkConfig is a process-global OnceCell. Isolate its mainnet setting
    // from the existing unit tests, which sign for devnet.
    const CHILD: &str = "P6D_MAINNET_SIGNATURE_CHECK";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "transaction_pool::ocaml_tests::resource::generated_commands_have_valid_mainnet_signatures", "--nocapture"])
            .env(CHILD, "1")
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    mina_core::NetworkConfig::init("mainnet").unwrap();
    for cmd in [payment(0, 0, MINA, FEE), zkapp(0, 15, 0, FEE, false, false)] {
        // Recompute all hashes from the wire, so cached fixture digests cannot
        // make this check pass with stale account-update commitments.
        let wire = v2::MinaBaseUserCommandStableV2::from(&cmd.data.forget_check());
        let decoded = UserCommand::try_from(&wire).unwrap();
        let verifiable = decoded
            .to_verifiable(&Applied, |_, _| Err("fixture does not use proofs".into()))
            .unwrap();
        let result = crate::verifier::common::check(WithStatus {
            data: verifiable,
            status: Applied,
        });
        assert!(
            matches!(result, crate::verifier::common::CheckResult::Valid(_)),
            "{result:?}"
        );
    }
}
