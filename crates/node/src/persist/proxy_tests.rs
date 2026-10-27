//! Tests of the web node's backend (I5e): a store whose files a file server
//! keeps, over an in-memory file system that fails as the browser's Origin
//! Private File System does. The browser's own file system is exercised by
//! the reload runs.

use std::{cell::RefCell, rc::Rc, thread};

use mina_core::block::ArcBlockWithHash;

use super::*;
use crate::persist::{
    event::PersistEvent,
    proxy::{
        block_on, serve, FsClient, MemFs, ProxyBackend, CHUNK, OPEN_ATTEMPTS, OPEN_RETRY_MS,
        SWEEP_AFTER_MS,
    },
    test_fixtures::mainnet_blocks,
    writer::{PersistJob, StoreWorker, WriterOptions},
};

const DAY_MS: u64 = 24 * 60 * 60 * 1000;
const NOW_MS: u64 = 1_760_000_000_000;

fn store_path(chain: &str) -> Vec<String> {
    vec!["mina-frontier".to_owned(), chain.to_owned()]
}

fn store_dir(chain: &str) -> String {
    store_path(chain).join("/")
}

/// A file server on a thread of its own, as the web node's runs on a worker,
/// with one client of it. The server stops when every client is gone.
fn server(fs: &MemFs) -> FsClient {
    let (client, calls) = FsClient::channel();
    let dir = fs.dir();
    thread::spawn(move || block_on(serve(dir, calls)));
    client
}

fn mounted(fs: &MemFs, chain: &str) -> ProxyBackend {
    ProxyBackend::mount(server(fs), store_path(chain), NOW_MS)
        .unwrap()
        .0
}

fn read_back(backend: &mut ProxyBackend, name: &str, piece: usize) -> Vec<u8> {
    let mut reader = backend.open(name).unwrap();
    let len = reader.len();
    let mut bytes = Vec::new();
    let mut buf = vec![0; piece];
    loop {
        match reader.read(&mut buf).unwrap() {
            0 => break,
            n => bytes.extend_from_slice(&buf[..n]),
        }
    }
    assert_eq!(bytes.len() as u64, len);
    bytes
}

#[test]
fn files_go_through_the_server_in_pieces() {
    let fs = MemFs::new();
    let mut backend = mounted(&fs, "chain");
    backend.lock().unwrap();

    // More than two pieces, written in uneven pieces.
    let mut rng = StdRng::seed_from_u64(5);
    let data = (0..2 * CHUNK + 12_345)
        .map(|_| rng.gen::<u8>())
        .collect::<Vec<_>>();
    let mut writer = backend.create("ledger-x.full").unwrap();
    let mut rest = data.as_slice();
    for piece in [1, 3, CHUNK - 1, 7, CHUNK + 5].into_iter().cycle() {
        if rest.is_empty() {
            break;
        }
        let piece = piece.min(rest.len());
        writer.write(&rest[..piece]).unwrap();
        rest = &rest[piece..];
    }
    let (len, crc32) = writer.finish().unwrap();
    assert_eq!(len, data.len() as u64);
    assert_eq!(crc32, crc32fast::hash(&data));
    // Finished files are closed on the server.
    assert!(!fs.is_open(&store_dir("chain"), "ledger-x.full"));
    assert_eq!(fs.files(&store_dir("chain"))["ledger-x.full"], data);

    for piece in [1, 4096, CHUNK, 3 * CHUNK] {
        assert!(read_back(&mut backend, "ledger-x.full", piece) == data);
    }
    // A reader dropped part way is closed too.
    let mut reader = backend.open("ledger-x.full").unwrap();
    reader.read_exact(&mut [0; 10]).unwrap();
    drop(reader);

    // Slots, listing, deleting, creating over a file.
    backend.write_slot("MANIFEST-A", b"first").unwrap();
    backend.write_slot("MANIFEST-A", b"2nd").unwrap();
    assert_eq!(read_back(&mut backend, "MANIFEST-A", 7), b"2nd");
    let mut writer = backend.create("ledger-x.full").unwrap();
    writer.write(b"short").unwrap();
    writer.finish().unwrap();
    assert_eq!(read_back(&mut backend, "ledger-x.full", 64), b"short");
    assert_eq!(
        backend.list().unwrap(),
        vec!["MANIFEST-A".to_owned(), "ledger-x.full".to_owned()]
    );
    backend.delete("ledger-x.full").unwrap();
    backend.delete("ledger-x.full").unwrap();
    assert_eq!(backend.list().unwrap(), vec!["MANIFEST-A".to_owned()]);
    assert_eq!(
        backend
            .open("ledger-x.full")
            .err()
            .map(|error| error.kind()),
        Some(io::ErrorKind::NotFound)
    );
    for name in ["", "..", "a/b"] {
        assert_eq!(
            backend.create(name).err().map(|error| error.kind()),
            Some(io::ErrorKind::InvalidInput)
        );
    }

    // A writer given up is closed without its last piece, so that the store
    // can delete the file.
    let mut writer = backend.create("block-y.bin").unwrap();
    writer.write(b"never flushed").unwrap();
    drop(writer);
    backend.delete("block-y.bin").unwrap();
    assert_eq!(backend.list().unwrap(), vec!["MANIFEST-A".to_owned()]);
    assert!(!fs.is_open(&store_dir("chain"), "block-y.bin"));
}

#[test]
fn a_file_another_worker_still_holds_is_retried_then_given_up() {
    let fs = MemFs::new();
    let mut backend = mounted(&fs, "chain");
    let path = format!("{}/MANIFEST-A", store_dir("chain"));

    // After a reload the old page's workers let go in a moment.
    fs.set_busy(&path, 2);
    backend.write_slot("MANIFEST-A", b"ok").unwrap();
    assert_eq!(fs.sleeps(), vec![OPEN_RETRY_MS, 2 * OPEN_RETRY_MS]);

    // A file held for good is an error, after the retries.
    fs.set_busy(&path, u32::MAX);
    let error = backend.write_slot("MANIFEST-A", b"no").unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ResourceBusy);
    assert_eq!(fs.sleeps().len() as u32, 2 + OPEN_ATTEMPTS - 1);
    assert_eq!(fs.files(&store_dir("chain"))["MANIFEST-A"], b"ok");
}

#[test]
fn a_second_tab_runs_without_the_store() {
    let fs = MemFs::new();
    let (first, _) = FrontierStore::open(mounted(&fs, "chain"), config()).unwrap();
    assert!(matches!(
        FrontierStore::open(mounted(&fs, "chain"), config()),
        Err(PersistError::Locked)
    ));
    // The first tab is closed: its workers' lock and files are let go.
    drop(first);
    fs.release_all();
    FrontierStore::open(mounted(&fs, "chain"), config()).unwrap();
}

#[test]
fn a_browser_without_the_file_system_opens_no_store() {
    let fs = MemFs::new();
    fs.set_mount_error(Some(io::ErrorKind::PermissionDenied));
    let error = ProxyBackend::mount(server(&fs), store_path("chain"), NOW_MS)
        .err()
        .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    // Nothing else works on a server with no store mounted.
    let (client, calls) = FsClient::channel();
    let dir = fs.dir();
    thread::spawn(move || block_on(serve(dir, calls)));
    let error = client.call(proxy::FsRequest::List).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::NotConnected);

    // As the store thread reports it: the node runs without a store.
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut worker = {
        let events = events.clone();
        let fs = fs.clone();
        StoreWorker::new(
            move |chain_id: &ChainId| {
                let (backend, _) =
                    ProxyBackend::mount(server(&fs), store_path(&chain_id.to_hex()), NOW_MS)?;
                Ok((backend, "opfs".to_owned()))
            },
            WriterOptions::default(),
            move |event| events.borrow_mut().push(event),
        )
    };
    worker.handle(PersistJob::Open {
        chain_id: MAINNET_CHAIN_ID,
        genesis_state_hash: genesis(),
    });
    match events.borrow().as_slice() {
        [PersistEvent::OpenFailed { error }] => {
            assert!(error.contains("no file system"), "{error}")
        }
        other => panic!("{other:?}"),
    };
}

#[test]
fn old_stores_of_other_chains_are_swept_at_the_mount() {
    let fs = MemFs::new();
    fs.set_now_ms(NOW_MS - SWEEP_AFTER_MS - DAY_MS);
    fs.put(&store_dir("old"), "MANIFEST-A", vec![1]);
    fs.put(&store_dir("old"), "block-a.bin", vec![2]);
    fs.put(&store_dir("mixed"), "MANIFEST-A", vec![3]);
    fs.set_now_ms(NOW_MS - DAY_MS);
    fs.put(&store_dir("mixed"), "MANIFEST-B", vec![4]);
    fs.put(&store_dir("recent"), "MANIFEST-A", vec![5]);
    // Not a store's file: outside the stores' directory.
    fs.put("elsewhere/old", "MANIFEST-A", vec![6]);
    fs.set_now_ms(NOW_MS);

    let (_, swept) = ProxyBackend::mount(server(&fs), store_path("current"), NOW_MS).unwrap();
    assert_eq!(swept, vec!["old".to_owned()]);
    assert!(fs.files(&store_dir("old")).is_empty());
    assert!(!fs.dirs().contains(&store_dir("old")));
    for kept in ["mixed", "recent", "current"] {
        assert!(fs.dirs().contains(&store_dir(kept)), "{kept}");
    }
    assert_eq!(fs.files(&store_dir("mixed")).len(), 2);
    assert_eq!(fs.files("elsewhere/old").len(), 1);

    // A store in use in another tab can't be deleted: the mount goes on.
    fs.set_now_ms(NOW_MS + SWEEP_AFTER_MS + DAY_MS);
    let mut recent = mounted(&fs, "recent");
    let _reader = recent.open("MANIFEST-A").unwrap();
    let (_, swept) = ProxyBackend::mount(
        server(&fs),
        store_path("current"),
        NOW_MS + 2 * SWEEP_AFTER_MS,
    )
    .unwrap();
    assert_eq!(swept, vec!["mixed".to_owned()]);
    assert!(fs.dirs().contains(&store_dir("recent")));
}

fn chain_of(blocks: &[ArcBlockWithHash]) -> Vec<ChainEntry> {
    blocks
        .iter()
        .map(|block| ChainEntry {
            hash: block.hash().clone(),
            height: block.height(),
            global_slot: block.global_slot(),
            just_emitted_a_proof: false,
        })
        .collect()
}

/// A browser's quota running out (a small disk, a private window) turns the
/// store off for the run, as a full disk does natively, and leaves no part
/// of the write that failed behind.
#[test]
fn a_full_quota_turns_the_store_off_and_leaves_no_partial_file() {
    let fs = MemFs::new();
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut worker = {
        let events = events.clone();
        let fs = fs.clone();
        StoreWorker::new(
            move |chain_id: &ChainId| {
                let (backend, _) =
                    ProxyBackend::mount(server(&fs), store_path(&chain_id.to_hex()), NOW_MS)?;
                Ok((backend, "opfs".to_owned()))
            },
            WriterOptions::default(),
            move |event| events.borrow_mut().push(event),
        )
    };
    let take = || events.borrow_mut().drain(..).collect::<Vec<_>>();
    worker.handle(PersistJob::Open {
        chain_id: MAINNET_CHAIN_ID,
        genesis_state_hash: genesis(),
    });
    assert!(matches!(take().as_slice(), [PersistEvent::Opened(_)]));

    let blocks = mainnet_blocks();
    let dir = store_dir(&MAINNET_CHAIN_ID.to_hex());
    worker.handle(PersistJob::ChainPut {
        chain: chain_of(&blocks[..1]),
        blocks: blocks[..1].to_vec(),
    });
    assert!(matches!(take().as_slice(), [PersistEvent::ChainPut(Ok(_))]));
    let before = fs.files(&dir);
    let used = before.values().map(|bytes| bytes.len() as u64).sum::<u64>();

    // Room for less than the second block.
    fs.set_quota(Some(used + 1000));
    worker.handle(PersistJob::ChainPut {
        chain: chain_of(&blocks),
        blocks: blocks.clone(),
    });
    match take().as_slice() {
        [PersistEvent::ChainPut(Err(error))] => {
            assert!(error.disable, "{}", error.error);
            assert!(error.error.contains("QuotaExceeded"), "{}", error.error);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(fs.files(&dir), before);
}
