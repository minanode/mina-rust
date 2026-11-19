//! The pool keeps one copy of each command, shared by every map that holds it,
//! and keeps that sharing across a serde round trip.

use super::*;

/// Each occurrence of each command in the pool's maps and in the locally
/// generated tables, by hash: the address of the command it points to.
fn occurrences(pool: &TransactionPool) -> BTreeMap<v2::TransactionHash, Vec<*const ()>> {
    let mut found: BTreeMap<_, Vec<_>> = BTreeMap::new();
    let mut add = |cmd: &ValidCommandWithHash| {
        found
            .entry(cmd.hash.clone())
            .or_default()
            .push(std::ptr::from_ref(cmd).cast::<()>());
    };
    let p = &pool.pool;
    p.all_by_sender
        .values()
        .flat_map(|(q, _)| q)
        .for_each(&mut add);
    p.all_by_fee.values().flatten().for_each(&mut add);
    p.applicable_by_fee.values().flatten().for_each(&mut add);
    p.transactions_with_expiration
        .values()
        .flatten()
        .for_each(&mut add);
    p.all_by_hash.values().for_each(&mut add);
    for table in [
        &pool.locally_generated_uncommitted,
        &pool.locally_generated_committed,
    ] {
        table.keys().map(|cmd| &**cmd).for_each(&mut add);
    }
    found
}

/// Every occurrence of a command points to one allocation.
fn assert_shared(pool: &TransactionPool) {
    for (hash, pointers) in occurrences(pool) {
        let first = pointers[0];
        assert!(
            pointers.iter().all(|p| *p == first),
            "{hash:?} has {} allocations",
            pointers.iter().collect::<BTreeSet<_>>().len()
        );
    }
}

fn hashes_by<K: Ord + Clone>(
    map: &HashMap<K, CommandSet>,
) -> BTreeMap<K, BTreeSet<v2::TransactionHash>> {
    map.iter()
        .map(|(key, cmds)| (key.clone(), hashes(cmds)))
        .collect()
}

/// Everything the pool holds, in a form that compares equal for equal pools.
#[derive(Debug, PartialEq)]
struct Contents {
    applicable_by_fee: BTreeMap<FeeRate, BTreeSet<v2::TransactionHash>>,
    all_by_sender: BTreeMap<AccountId, (Vec<v2::TransactionHash>, Amount)>,
    all_by_fee: BTreeMap<FeeRate, BTreeSet<v2::TransactionHash>>,
    /// Each command's encoding: a deserialized zkApp command's hash cells are
    /// empty until used, and `PartialEq` needs them.
    commands: BTreeMap<v2::TransactionHash, Vec<u8>>,
    transactions_with_expiration: BTreeMap<Slot, BTreeSet<v2::TransactionHash>>,
    size: usize,
    slot_tx_end: Option<Slot>,
    uncommitted: BTreeMap<v2::TransactionHash, (redux::Timestamp, Batch)>,
    committed: BTreeMap<v2::TransactionHash, (redux::Timestamp, Batch)>,
    current_batch: usize,
    remaining_in_batch: usize,
    pool_max_size: usize,
    verification_keys: BTreeMap<String, usize>,
    account_id_to_vks: BTreeMap<AccountId, BTreeMap<String, usize>>,
}

fn contents(pool: &TransactionPool) -> Contents {
    let p = &pool.pool;
    let table = |t: &HashMap<PooledCommand, (redux::Timestamp, Batch)>| {
        t.iter()
            .map(|(cmd, value)| (cmd.hash.clone(), value.clone()))
            .collect()
    };
    let vks = &pool.verification_key_table;
    Contents {
        applicable_by_fee: hashes_by(&p.applicable_by_fee),
        all_by_sender: p
            .all_by_sender
            .iter()
            .map(|(id, (q, amount))| {
                (
                    id.clone(),
                    (q.iter().map(|c| c.hash.clone()).collect(), *amount),
                )
            })
            .collect(),
        all_by_fee: hashes_by(&p.all_by_fee),
        commands: p
            .all_by_hash
            .values()
            .map(|cmd| (cmd.hash.clone(), postcard::to_allocvec(&cmd.data).unwrap()))
            .collect(),
        transactions_with_expiration: hashes_by(&p.transactions_with_expiration),
        size: p.size,
        slot_tx_end: p.config.slot_tx_end,
        uncommitted: table(&pool.locally_generated_uncommitted),
        committed: table(&pool.locally_generated_committed),
        current_batch: pool.current_batch,
        remaining_in_batch: pool.remaining_in_batch,
        pool_max_size: pool.config.pool_max_size,
        verification_keys: vks
            .verification_keys
            .iter()
            .map(|(hash, (count, _))| (hash.to_string(), *count))
            .collect(),
        account_id_to_vks: vks
            .account_id_to_vks
            .iter()
            .map(|(id, map)| {
                (
                    id.clone(),
                    map.iter()
                        .map(|(hash, count)| (hash.to_string(), *count))
                        .collect(),
                )
            })
            .collect(),
    }
}

/// A zkApp command that sets a verification key, as the resource tests build.
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

fn round_trip(pool: &TransactionPool) -> TransactionPool {
    let bytes = postcard::to_allocvec(pool).unwrap();
    postcard::from_bytes(&bytes).unwrap()
}

/// Sender 0 has one applicable command and two queued behind it, all
/// expiring; sender 1 a zkApp command with a verification key; sender 2 a
/// payment that does not expire. All are local, so the locally generated
/// table holds them too.
fn mixed_pool() -> (Harness, Vec<ValidCommandWithHash>) {
    let mut h = Harness::new();
    let cmds = vec![
        payment_until(0, 0, MINA, FEE, 100),
        payment_until(0, 1, MINA, FEE, 100),
        payment_until(0, 2, MINA, FEE, 200),
        with_vk(1, 0),
        payment(2, 0, MINA, 2 * FEE),
    ];
    h.add(&cmds, true);
    (h, cmds)
}

#[test]
fn command_in_several_maps_is_one_allocation() {
    let (h, cmds) = mixed_pool();
    let pool = &h.pool.pool;
    assert_eq!(pool.size(), cmds.len());

    // Queue heads are applicable; the queued commands are not.
    let applicable = hashes(pool.applicable_by_fee.values().flatten());
    assert!(applicable.contains(&cmds[0].hash));
    assert!(!applicable.contains(&cmds[1].hash));
    assert!(!applicable.contains(&cmds[2].hash));

    let occurrences = occurrences(&h.pool);
    // Head with an expiry: sender, by fee, applicable, expiration, by hash
    // and the locally generated table.
    assert_eq!(occurrences[&cmds[0].hash].len(), 6);
    // Queued with an expiry: no applicable entry.
    assert_eq!(occurrences[&cmds[1].hash].len(), 5);
    // Head without an expiry.
    assert_eq!(occurrences[&cmds[4].hash].len(), 5);
    assert_shared(&h.pool);

    // One allocation, pointed to by exactly those occurrences.
    for cmd in &cmds {
        let shared = pool.all_by_hash.get_shared(&cmd.hash).unwrap();
        assert_eq!(
            std::sync::Arc::strong_count(shared),
            occurrences[&cmd.hash].len()
        );
    }
}

#[test]
fn listing_and_dropping_keep_one_allocation() {
    let (mut h, cmds) = mixed_pool();
    // Listing for a block works on copies of the maps: they point to the
    // pool's commands.
    let listed = h.pool.pool.list_includable_transactions(10);
    assert_eq!(listed.len(), cmds.len());
    for cmd in listed.iter_shared() {
        let pooled = h.pool.pool.all_by_hash.get_shared(&cmd.hash).unwrap();
        assert!(std::sync::Arc::ptr_eq(cmd, pooled));
    }
    drop(listed);

    // A replacement drops the queued tail and shares the new command.
    let replacement = payment_until(0, 1, MINA, 3 * FEE, 100);
    h.add(std::slice::from_ref(&replacement), true);
    assert_shared(&h.pool);
    let occurrences = occurrences(&h.pool);
    assert_eq!(occurrences[&replacement.hash].len(), 5);
    assert!(!occurrences.contains_key(&cmds[1].hash));
}

#[test]
fn serde_round_trip_keeps_pool_and_sharing() {
    let (mut h, cmds) = mixed_pool();
    // Commit sender 2's payment, so the committed table holds a command the
    // pool no longer has.
    h.commit(&cmds[4..]);
    assert_eq!(h.pool.size(), cmds.len() - 1);
    assert_eq!(h.pool.locally_generated_committed.len(), 1);

    let mut restored = round_trip(&h.pool);
    assert_eq!(contents(&restored), contents(&h.pool));
    assert_shared(&restored);
    assert_pool_consistency(&restored.pool);
    // The committed command is not in the pool: it keeps its own allocation.
    assert_eq!(
        occurrences(&restored)[&cmds[4].hash].len(),
        1,
        "{:?}",
        contents(&restored).committed
    );

    // A second round trip gives the same pool again.
    assert_eq!(contents(&round_trip(&restored)), contents(&h.pool));

    // The restored pool works: apply the same operations to both pools.
    std::mem::swap(&mut h.pool, &mut restored);
    let mut original = Harness {
        pool: restored,
        accounts: h.accounts.clone(),
        slot: h.slot,
    };
    for h in [&mut h, &mut original] {
        let more = [
            payment_until(0, 3, MINA, FEE, 200),
            payment(3, 0, MINA, FEE),
        ];
        h.add(&more, true);
        h.commit(&cmds[..1]);
        h.pool.on_new_best_tip(Slot::zero(), &h.accounts).unwrap();
        assert_shared(&h.pool);
    }
    assert_eq!(contents(&h.pool), contents(&original.pool));
    assert_eq!(
        h.pool.list_includable_transactions(10),
        original.pool.list_includable_transactions(10)
    );
    let rebroadcastable = |pool: &mut TransactionPool| {
        pool.get_rebroadcastable(|_| false)
            .into_iter()
            .map(|batch| {
                batch
                    .into_iter()
                    .map(|c| transaction_hash::hash_command(c.to_valid_unsafe()).hash)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        rebroadcastable(&mut h.pool),
        rebroadcastable(&mut original.pool)
    );
}
