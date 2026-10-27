//! The CLI's default identity. Explicit keys never touch this file.

use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

use anyhow::Context;
use mina_node::p2p::identity::SecretKey;

const KEY_FILE: &str = "p2p-secret-key";

pub(crate) fn resolve(work_dir: &Path, explicit: Option<SecretKey>) -> anyhow::Result<SecretKey> {
    if let Some(key) = explicit {
        return Ok(key);
    }
    fs::create_dir_all(work_dir).context("create identity directory")?;
    let path = work_dir.join(KEY_FILE);
    match load(&path) {
        Ok(key) => return Ok(key),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error).context("load saved p2p identity"),
    }

    let key = SecretKey::rand();
    // Publish only a complete, synced file, without replacing an existing
    // identity. Concurrent creators load the winner, never rotate its key.
    let mut file = tempfile::NamedTempFile::new_in(work_dir).context("create p2p identity")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(&key.to_bytes())
        .context("write p2p identity")?;
    file.as_file().sync_all().context("sync p2p identity")?;
    match file.persist_noclobber(&path) {
        Ok(_) => {
            #[cfg(unix)]
            fs::File::open(work_dir)?.sync_all()?;
            Ok(key)
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            load(&path).context("load concurrently created p2p identity")
        }
        Err(error) => Err(error.error).context("save p2p identity"),
    }
}

fn load(path: &Path) -> std::io::Result<SecretKey> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Do not follow symlinks or block on a FIFO in place of the key.
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    let mut file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() != 32 {
        return Err(std::io::Error::other(
            "p2p identity must be a 32-byte regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o600 {
            return Err(std::io::Error::other(
                "p2p identity permissions must be 0600",
            ));
        }
    }
    let mut bytes = [0; 32];
    file.read_exact(&mut bytes)?;
    Ok(SecretKey::from_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_survives_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let first = resolve(dir.path(), None).unwrap().public_key();
        let second = resolve(dir.path(), None).unwrap().public_key();
        assert_eq!(first, second);
    }

    #[test]
    fn explicit_identity_wins_without_reading_or_changing_saved_key() {
        let dir = tempfile::tempdir().unwrap();
        let saved = resolve(dir.path(), None).unwrap().public_key();
        let explicit = SecretKey::rand();
        assert_eq!(
            resolve(dir.path(), Some(explicit.clone()))
                .unwrap()
                .public_key(),
            explicit.public_key()
        );
        assert_eq!(resolve(dir.path(), None).unwrap().public_key(), saved);
        fs::write(dir.path().join(KEY_FILE), []).unwrap();
        assert!(resolve(dir.path(), Some(explicit)).is_ok());
        assert!(resolve(dir.path(), None).is_err());
    }

    #[test]
    fn concurrent_creators_share_one_complete_identity() {
        let dir = tempfile::tempdir().unwrap();
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        resolve(dir.path(), None).unwrap().public_key()
                    })
                })
                .collect();
            let keys: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            assert!(keys.iter().all(|key| key == &keys[0]));
        });
    }

    #[cfg(unix)]
    #[test]
    fn identity_is_private_and_refuses_unsafe_existing_files() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        resolve(dir.path(), None).unwrap();
        let path = dir.path().join(KEY_FILE);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(resolve(dir.path(), None).is_err());
        fs::remove_file(&path).unwrap();
        symlink(dir.path().join("missing"), &path).unwrap();
        assert!(resolve(dir.path(), None).is_err());
        assert!(!dir.path().join("missing").exists());
    }
}
