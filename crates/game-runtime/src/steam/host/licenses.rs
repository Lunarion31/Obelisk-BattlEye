//! What the Steam client keeps on disk between sessions: the licenses it
//! received (`licensecache`), the package information it fetched
//! (`packageinfo.vdf`) and the information of the apps they hold
//! (`appinfo.vdf`). Steam reads them at start and asks its servers only for
//! what changed since, so it knows the account's games before the network has
//! answered. A host's home is thrown away with its launch, so each launch
//! began cold and a large library could take over a minute to publish its
//! licenses. Obelisk keeps these files for the account. All three matter: the
//! wait between sign-in and a game reported owned fell from about three seconds
//! to under one only once `appinfo.vdf` was kept too. Ownership still comes
//! from the ticket the servers issue, never from these files.
use super::files::atomic_write;
use crate::{files::private_directory, steam::Error};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// A kept account's folder in the runtime cache, which the cleanup of stale
/// launches leaves alone.
const ACCOUNT: &str = "account-";

/// The files the client keeps for `account`: the name each has among the
/// kept files, and where the client reads and writes it under its Steam
/// directory.
fn files(account: u32) -> [(&'static str, String); 3] {
    [
        ("packageinfo.vdf", "appcache/packageinfo.vdf".into()),
        ("appinfo.vdf", "appcache/appinfo.vdf".into()),
        (
            "licensecache",
            format!("userdata/{account}/config/licensecache"),
        ),
    ]
}

/// One account's kept files.
pub(crate) struct LicenseCache {
    kept: PathBuf,
    account: u32,
}
impl LicenseCache {
    pub fn new(cache: &Path, steam_id: u64) -> Result<Self, Error> {
        let kept = cache.join(format!("{ACCOUNT}{steam_id}"));
        private_directory(&kept)?;
        Ok(Self {
            kept,
            account: steam_id as u32,
        })
    }
    /// The folders the client writes its files in, under `steam`.
    pub fn folders(&self, steam: &Path) -> [PathBuf; 2] {
        [
            steam.join("appcache"),
            steam.join(format!("userdata/{}/config", self.account)),
        ]
    }
    /// Gives the client the kept files it has none of yet, and the folders to
    /// write them in. A file already there is newer than the kept one.
    pub fn seed(&self, steam: &Path) -> Result<(), Error> {
        for folder in self.folders(steam) {
            private_directory(&folder)?;
        }
        for (name, place) in files(self.account) {
            let (kept, target) = (self.kept.join(name), steam.join(place));
            if kept.is_file() && !target.exists() {
                fs::copy(kept, target)?;
            }
        }
        Ok(())
    }
    /// Keeps the client's files, replacing the kept ones whole. What the client
    /// left unfinished fails its own checks when read back, as it would after
    /// a crash of Steam itself.
    pub fn keep(&self, steam: &Path) {
        for (name, place) in files(self.account) {
            let source = steam.join(place);
            // The client's folder is its own: follow nothing it linked.
            let Ok(metadata) = fs::symlink_metadata(&source) else {
                continue;
            };
            if !metadata.is_file() || metadata.len() == 0 {
                continue;
            }
            let _ = atomic_write(&self.kept.join(name), |file| {
                std::io::copy(&mut fs::File::open(&source)?, file)?;
                Ok(())
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client_directory(root: &Path) -> PathBuf {
        root.join("home/Library/Application Support/Steam")
    }
    const ID: u64 = 0x011000010000002a;
    fn client_files(steam: &Path) -> [PathBuf; 3] {
        files(42).map(|(_, place)| steam.join(place))
    }

    #[test]
    fn a_cold_host_gets_the_kept_files_and_the_folders_to_write_in() {
        let root = tempfile::tempdir().unwrap();
        let cache = LicenseCache::new(root.path(), ID).unwrap();
        fs::write(cache.kept.join("licensecache"), b"licenses").unwrap();
        let steam = client_directory(root.path());
        cache.seed(&steam).unwrap();
        let [packages, _, licenses] = client_files(&steam);
        // Only what was kept: the client writes the rest.
        assert!(!packages.exists());
        assert_eq!(fs::read(&licenses).unwrap(), b"licenses");
        assert!(packages.parent().unwrap().is_dir());
    }

    #[test]
    fn seeding_never_replaces_a_file_the_client_already_has() {
        let root = tempfile::tempdir().unwrap();
        let cache = LicenseCache::new(root.path(), ID).unwrap();
        fs::write(cache.kept.join("packageinfo.vdf"), b"older").unwrap();
        let steam = client_directory(root.path());
        let [packages, ..] = client_files(&steam);
        fs::create_dir_all(packages.parent().unwrap()).unwrap();
        fs::write(&packages, b"newer").unwrap();
        cache.seed(&steam).unwrap();
        assert_eq!(fs::read(packages).unwrap(), b"newer");
    }

    #[test]
    fn what_the_client_wrote_is_kept_for_the_next_host_of_the_account() {
        let root = tempfile::tempdir().unwrap();
        let cache = LicenseCache::new(root.path(), ID).unwrap();
        let steam = client_directory(root.path());
        cache.seed(&steam).unwrap();
        let [packages, _, licenses] = client_files(&steam);
        fs::write(&packages, b"packages").unwrap();
        fs::write(&licenses, b"licenses").unwrap();
        cache.keep(&steam);
        fs::write(&packages, b"more packages").unwrap();
        cache.keep(&steam);

        let next = LicenseCache::new(root.path(), ID).unwrap();
        let other = tempfile::tempdir().unwrap();
        let fresh = client_directory(other.path());
        next.seed(&fresh).unwrap();
        let [packages, _, licenses] = client_files(&fresh);
        assert_eq!(fs::read(packages).unwrap(), b"more packages");
        assert_eq!(fs::read(licenses).unwrap(), b"licenses");
        // Another account of the same Mac starts from nothing.
        let stranger = LicenseCache::new(root.path(), ID + 1).unwrap();
        assert_ne!(stranger.kept, next.kept);
        assert_eq!(fs::read_dir(&stranger.kept).unwrap().count(), 0);
    }

    #[test]
    fn an_empty_or_linked_file_never_replaces_a_kept_one() {
        let root = tempfile::tempdir().unwrap();
        let cache = LicenseCache::new(root.path(), ID).unwrap();
        fs::write(cache.kept.join("licensecache"), b"licenses").unwrap();
        fs::write(cache.kept.join("packageinfo.vdf"), b"packages").unwrap();
        let steam = client_directory(root.path());
        cache.seed(&steam).unwrap();
        let [packages, _, licenses] = client_files(&steam);
        fs::write(&licenses, b"").unwrap();
        fs::remove_file(&packages).unwrap();
        let secret = root.path().join("secret");
        fs::write(&secret, b"not the client's").unwrap();
        std::os::unix::fs::symlink(&secret, &packages).unwrap();
        cache.keep(&steam);
        assert_eq!(
            fs::read(cache.kept.join("licensecache")).unwrap(),
            b"licenses"
        );
        assert_eq!(
            fs::read(cache.kept.join("packageinfo.vdf")).unwrap(),
            b"packages"
        );
    }
}
