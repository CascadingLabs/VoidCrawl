//! VoidCrawl-owned Chromium profile registry.
//!
//! Managed profiles are standalone Chrome `user_data_dir` roots below
//! `$VOIDCRAWL_PROFILE_ROOT` (or the platform data-dir default). They are not
//! subprofiles inside the user's daily Chrome data directory.

use std::{
    collections::BTreeMap,
    env, fmt, fs,
    fs::{File, OpenOptions},
    io::ErrorKind,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use fd_lock::{RwLock as FileLock, RwLockWriteGuard};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    error::{Result, VoidCrawlError},
    lease::{read_metadata, write_metadata},
    profile::{expand_tilde, resolve_profile},
};

const MANIFEST_FILE: &str = "registry.json";
/// Guardrail against accidentally multiplying a large profile without bound.
pub const MAX_PROFILE_SPLIT_COPIES: usize = 16;

#[derive(Debug, Clone)]
pub struct ProfileRegistry {
    root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ManagedProfile {
    pub id:           String,
    pub path:         PathBuf,
    pub created_at:   u64,
    pub last_used_at: Option<u64>,
    pub labels:       Vec<String>,
    pub description:  Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ManagedProfileDescription {
    #[serde(flatten)]
    pub profile: ManagedProfile,
    pub size:    u64,
    pub status:  ProfileStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProfileStatus {
    Available,
    Locked,
    Missing,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ProfilePool {
    pub name:        String,
    pub profile_ids: Vec<String>,
    pub max_active:  usize,
    pub round_robin: bool,
    #[serde(default)]
    pub next_index:  usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ResolvedProfilePool {
    pub pool:     ProfilePool,
    pub profiles: Vec<ManagedProfileDescription>,
}

pub struct ManagedProfileLease {
    id:    String,
    path:  PathBuf,
    _lock: RwLockWriteGuard<'static, File>,
}

/// Temporary, isolated clone of a quiesced managed profile.
/// The directory is removed when this lease is dropped.
pub struct ManagedProfileSnapshot {
    source_id: String,
    path:      PathBuf,
    _tempdir:  tempfile::TempDir,
}

impl fmt::Debug for ManagedProfileSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManagedProfileSnapshot")
            .field("source_id", &self.source_id)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl ManagedProfileSnapshot {
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl fmt::Debug for ManagedProfileLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManagedProfileLease")
            .field("id", &self.id)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl ManagedProfileLease {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Manifest {
    #[serde(default)]
    profiles: BTreeMap<String, ManagedProfile>,
    #[serde(default)]
    pools:    BTreeMap<String, ProfilePool>,
}

impl Default for ProfileRegistry {
    fn default() -> Self {
        Self { root: default_profile_root() }
    }
}

impl ProfileRegistry {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn create_profile(
        &self,
        id: &str,
        description: Option<String>,
        labels: Vec<String>,
    ) -> Result<ManagedProfileDescription> {
        validate_name(id, "profile id")?;
        self.with_manifest_write(|manifest| {
            if manifest.profiles.contains_key(id) {
                return Err(VoidCrawlError::Other(format!(
                    "managed profile {id:?} already exists"
                )));
            }
            let path = self.profile_path(id);
            fs::create_dir_all(&path).map_err(|e| {
                VoidCrawlError::Other(format!("create profile dir {}: {e}", path.display()))
            })?;
            seed_standalone_profile(&path)?;
            let profile = ManagedProfile {
                id: id.to_string(),
                path: path.clone(),
                created_at: now_epoch_secs(),
                last_used_at: None,
                labels,
                description,
            };
            manifest.profiles.insert(id.to_string(), profile.clone());
            Self::describe_existing(profile)
        })
    }

    pub fn clone_profile(
        &self,
        source_id_or_path: &str,
        id: &str,
        description: Option<String>,
        labels: Vec<String>,
    ) -> Result<ManagedProfileDescription> {
        validate_name(id, "profile id")?;
        self.with_manifest_write(|manifest| {
            if manifest.profiles.contains_key(id) {
                return Err(VoidCrawlError::Other(format!(
                    "managed profile {id:?} already exists"
                )));
            }
            let source = manifest
                .profiles
                .get(source_id_or_path)
                .map_or_else(|| PathBuf::from(expand_tilde(source_id_or_path)), |p| p.path.clone());
            if !source.is_dir() {
                return Err(VoidCrawlError::ProfileNotFound {
                    name:     source_id_or_path.to_string(),
                    searched: vec![self.root.display().to_string()],
                });
            }
            let path = self.profile_path(id);
            copy_dir_recursively(&source, &path)?;
            let profile = ManagedProfile {
                id: id.to_string(),
                path: path.clone(),
                created_at: now_epoch_secs(),
                last_used_at: None,
                labels,
                description,
            };
            manifest.profiles.insert(id.to_string(), profile.clone());
            Self::describe_existing(profile)
        })
    }

    /// Create a uniquely named temporary snapshot while holding the source's
    /// authoritative VoidCrawl lease. Lock and Chrome Singleton files are not
    /// copied. The returned directory is deleted on drop.
    pub fn snapshot_profile(&self, id: &str) -> Result<ManagedProfileSnapshot> {
        self.snapshot_copies(id, 1)?
            .pop()
            .ok_or_else(|| VoidCrawlError::Other("profile snapshot was not created".into()))
    }

    /// Fork an installed native Chrome profile into isolated, concurrently
    /// runnable `user_data_dir` roots.
    ///
    /// `source_name_or_path` may be a discovered Chrome profile name such as
    /// `Default` or an explicit profile-directory path. The containing Chrome
    /// user-data root must not be running: copying live SQLite and LevelDB
    /// state cannot produce a consistent fork. The native profile is
    /// normalized to `Default` inside each result, and its root `Local
    /// State` file is copied so encrypted cookies and profile metadata
    /// retain their Chrome context.
    pub fn fork_profile(
        &self,
        source_name_or_path: &str,
        copies: usize,
    ) -> Result<Vec<ManagedProfileSnapshot>> {
        validate_split_copies(copies)?;
        let explicit = PathBuf::from(expand_tilde(source_name_or_path));
        let source =
            if explicit.is_dir() { explicit } else { resolve_profile(source_name_or_path)? };
        if !source.join("Preferences").is_file() {
            return Err(VoidCrawlError::ProfileNotFound {
                name:     source_name_or_path.to_string(),
                searched: vec![source.display().to_string()],
            });
        }
        let user_data_root = source.parent().ok_or_else(|| {
            VoidCrawlError::Other(format!(
                "native profile {} has no Chrome user-data parent",
                source.display()
            ))
        })?;
        let singleton_lock = user_data_root.join("SingletonLock");
        if singleton_lock.symlink_metadata().is_ok() {
            return Err(VoidCrawlError::ChromeProfileBusy {
                name:      source_name_or_path.to_string(),
                lock_path: singleton_lock.display().to_string(),
            });
        }

        let _source_lease = acquire_profile_lock(source_name_or_path, &source)?;
        self.copy_profile_baseline(source_name_or_path, copies, |destination| {
            let default_profile = destination.join("Default");
            copy_snapshot_recursively(&source, &default_profile)?;
            copy_regular_file_if_present(
                &user_data_root.join("Local State"),
                &destination.join("Local State"),
            )
        })
    }

    /// Split one quiesced managed profile into isolated, concurrently runnable
    /// copies from the same baseline.
    ///
    /// The source lease is held across every copy, so no cooperating VoidCrawl
    /// process can modify the source between copy one and copy N. Each returned
    /// snapshot has a unique `user_data_dir`; Chrome therefore gives every
    /// worker its own `SingletonLock`. The copies begin with the same cookies,
    /// storage, extensions, and profile identity, but writes made after launch
    /// are intentionally not synchronized between them or back to the source.
    /// Dropping the returned snapshots deletes all temporary directories.
    pub fn split_profile(&self, id: &str, copies: usize) -> Result<Vec<ManagedProfileSnapshot>> {
        validate_split_copies(copies)?;
        self.snapshot_copies(id, copies)
    }

    fn snapshot_copies(&self, id: &str, copies: usize) -> Result<Vec<ManagedProfileSnapshot>> {
        let profile = self.describe_profile(id)?.profile;
        let _source_lease = acquire_profile_lock(id, &profile.path)?;
        self.copy_profile_baseline(id, copies, |destination| {
            copy_snapshot_recursively(&profile.path, destination)
        })
    }

    fn copy_profile_baseline(
        &self,
        source_id: &str,
        copies: usize,
        mut copy_into: impl FnMut(&Path) -> Result<()>,
    ) -> Result<Vec<ManagedProfileSnapshot>> {
        let snapshots_root = self.root.join(".snapshots");
        fs::create_dir_all(&snapshots_root).map_err(|e| {
            VoidCrawlError::Other(format!("create snapshot root {}: {e}", snapshots_root.display()))
        })?;

        let mut snapshots = Vec::with_capacity(copies);
        for _ in 0..copies {
            let tempdir = tempfile::Builder::new()
                .prefix(&format!("{source_id}-"))
                .tempdir_in(&snapshots_root)
                .map_err(|e| VoidCrawlError::Other(format!("create profile snapshot: {e}")))?;
            let path = tempdir.path().join("profile");
            copy_into(&path)?;
            snapshots.push(ManagedProfileSnapshot {
                source_id: source_id.to_string(),
                path,
                _tempdir: tempdir,
            });
        }
        Ok(snapshots)
    }

    pub fn list_profiles(&self) -> Result<Vec<ManagedProfileDescription>> {
        let manifest = self.load_manifest()?;
        manifest.profiles.into_values().map(Self::describe_existing).collect()
    }

    pub fn describe_profile(&self, id: &str) -> Result<ManagedProfileDescription> {
        let manifest = self.load_manifest()?;
        let profile =
            manifest.profiles.get(id).cloned().ok_or_else(|| VoidCrawlError::ProfileNotFound {
                name:     id.to_string(),
                searched: vec![self.root.display().to_string()],
            })?;
        Self::describe_existing(profile)
    }

    pub fn delete_profile(&self, id: &str) -> Result<bool> {
        self.with_manifest_write(|manifest| {
            let Some(profile) = manifest.profiles.get(id).cloned() else {
                return Ok(false);
            };
            if matches!(lock_status(&profile.path)?, ProfileStatus::Locked) {
                return Err(profile_busy(id, &profile.path.join(".voidcrawl.lock")));
            }
            if profile.path.exists() {
                fs::remove_dir_all(&profile.path).map_err(|e| {
                    VoidCrawlError::Other(format!(
                        "delete profile dir {}: {e}",
                        profile.path.display()
                    ))
                })?;
            }
            manifest.profiles.remove(id);
            for pool in manifest.pools.values_mut() {
                pool.profile_ids.retain(|profile_id| profile_id != id);
                if pool.profile_ids.is_empty() {
                    pool.next_index = 0;
                } else {
                    pool.next_index %= pool.profile_ids.len();
                }
            }
            Ok(true)
        })
    }

    pub fn create_pool(
        &self,
        name: &str,
        profile_ids: Vec<String>,
        max_active: usize,
    ) -> Result<ProfilePool> {
        validate_name(name, "pool name")?;
        if profile_ids.is_empty() {
            return Err(VoidCrawlError::Other("profile pool requires at least one profile".into()));
        }
        self.with_manifest_write(|manifest| {
            for profile_id in &profile_ids {
                if !manifest.profiles.contains_key(profile_id) {
                    return Err(VoidCrawlError::ProfileNotFound {
                        name:     profile_id.clone(),
                        searched: vec![self.root.display().to_string()],
                    });
                }
            }
            let pool = ProfilePool {
                name: name.to_string(),
                profile_ids,
                max_active: max_active.max(1),
                round_robin: true,
                next_index: 0,
            };
            manifest.pools.insert(name.to_string(), pool.clone());
            Ok(pool)
        })
    }

    pub fn list_pools(&self) -> Result<Vec<ProfilePool>> {
        let manifest = self.load_manifest()?;
        Ok(manifest.pools.into_values().collect())
    }

    pub fn resolve_pool(&self, name: &str) -> Result<ResolvedProfilePool> {
        let manifest = self.load_manifest()?;
        let pool = manifest.pools.get(name).cloned().ok_or_else(|| {
            VoidCrawlError::Other(format!("managed profile pool {name:?} not found"))
        })?;
        let mut profiles = Vec::with_capacity(pool.profile_ids.len());
        for id in &pool.profile_ids {
            let profile = manifest.profiles.get(id).cloned().ok_or_else(|| {
                VoidCrawlError::ProfileNotFound {
                    name:     id.clone(),
                    searched: vec![self.root.display().to_string()],
                }
            })?;
            profiles.push(Self::describe_existing(profile)?);
        }
        Ok(ResolvedProfilePool { pool, profiles })
    }

    pub fn acquire_profile(&self, id: &str) -> Result<ManagedProfileLease> {
        self.with_manifest_write(|manifest| {
            let profile =
                manifest.profiles.get_mut(id).ok_or_else(|| VoidCrawlError::ProfileNotFound {
                    name:     id.to_string(),
                    searched: vec![self.root.display().to_string()],
                })?;
            let lease = acquire_profile_lock(&profile.id, &profile.path)?;
            profile.last_used_at = Some(now_epoch_secs());
            Ok(lease)
        })
    }

    pub fn acquire_from_pool(&self, name: &str) -> Result<ManagedProfileLease> {
        self.with_manifest_write(|manifest| {
            let pool = manifest.pools.get_mut(name).ok_or_else(|| {
                VoidCrawlError::Other(format!("managed profile pool {name:?} not found"))
            })?;
            if pool.profile_ids.is_empty() {
                return Err(VoidCrawlError::Other(format!(
                    "managed profile pool {name:?} is empty"
                )));
            }

            let active_cap = pool.max_active.max(1).min(pool.profile_ids.len());
            let start = if pool.round_robin { pool.next_index % pool.profile_ids.len() } else { 0 };
            let mut last_busy: Option<String> = None;
            for offset in 0..active_cap {
                let index = (start + offset) % pool.profile_ids.len();
                let id = pool.profile_ids[index].clone();
                let profile = manifest.profiles.get_mut(&id).ok_or_else(|| {
                    VoidCrawlError::ProfileNotFound {
                        name:     id.clone(),
                        searched: vec![self.root.display().to_string()],
                    }
                })?;
                match acquire_profile_lock(&profile.id, &profile.path) {
                    Ok(lease) => {
                        pool.next_index = (index + 1) % pool.profile_ids.len();
                        profile.last_used_at = Some(now_epoch_secs());
                        return Ok(lease);
                    }
                    Err(VoidCrawlError::ProfileBusy { name, .. }) => {
                        last_busy = Some(name);
                    }
                    Err(err) => return Err(err),
                }
            }
            Err(VoidCrawlError::ProfileBusy {
                name:        last_busy.unwrap_or_else(|| name.to_string()),
                pid:         None,
                acquired_at: None,
            })
        })
    }

    fn profile_path(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    fn manifest_path(&self) -> PathBuf {
        self.root.join(MANIFEST_FILE)
    }

    fn load_manifest(&self) -> Result<Manifest> {
        let path = self.manifest_path();
        if !path.exists() {
            return Ok(Manifest::default());
        }
        let raw = fs::read_to_string(&path)
            .map_err(|e| VoidCrawlError::Other(format!("read manifest {}: {e}", path.display())))?;
        serde_json::from_str(&raw).map_err(|e| {
            VoidCrawlError::Other(format!("parse managed profile manifest {}: {e}", path.display()))
        })
    }

    fn with_manifest_write<T>(&self, f: impl FnOnce(&mut Manifest) -> Result<T>) -> Result<T> {
        fs::create_dir_all(&self.root).map_err(|e| {
            VoidCrawlError::Other(format!(
                "create profile registry root {}: {e}",
                self.root.display()
            ))
        })?;
        let lock_path = self.root.join(".manifest.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| VoidCrawlError::Other(format!("open {}: {e}", lock_path.display())))?;
        let mut lock = FileLock::new(file);
        let _guard = lock
            .write()
            .map_err(|e| VoidCrawlError::Other(format!("lock {}: {e}", lock_path.display())))?;
        let mut manifest = self.load_manifest()?;
        let result = f(&mut manifest)?;
        let raw = serde_json::to_string_pretty(&manifest).map_err(|e| {
            VoidCrawlError::Other(format!("serialize managed profile manifest: {e}"))
        })?;
        let manifest_path = self.manifest_path();
        let tmp_path = manifest_path.with_extension("json.tmp");
        fs::write(&tmp_path, raw).map_err(|e| {
            VoidCrawlError::Other(format!("write manifest temp {}: {e}", tmp_path.display()))
        })?;
        fs::rename(&tmp_path, &manifest_path).map_err(|e| {
            VoidCrawlError::Other(format!(
                "replace manifest {} with {}: {e}",
                manifest_path.display(),
                tmp_path.display()
            ))
        })?;
        Ok(result)
    }

    fn describe_existing(profile: ManagedProfile) -> Result<ManagedProfileDescription> {
        let status = if profile.path.is_dir() {
            lock_status(&profile.path)?
        } else {
            ProfileStatus::Missing
        };
        let size = if profile.path.is_dir() { dir_size(&profile.path)? } else { 0 };
        Ok(ManagedProfileDescription { profile, size, status })
    }
}

pub fn default_profile_root() -> PathBuf {
    if let Ok(root) = env::var("VOIDCRAWL_PROFILE_ROOT") {
        return PathBuf::from(expand_tilde(&root));
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(data_home) =
            env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute())
        {
            return data_home.join("voidcrawl").join("profiles");
        }
        if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
            return home.join(".local").join("share").join("voidcrawl").join("profiles");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
            return home
                .join("Library")
                .join("Application Support")
                .join("voidcrawl")
                .join("profiles");
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(local) = env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            return local.join("voidcrawl").join("profiles");
        }
    }
    PathBuf::from(".voidcrawl").join("profiles")
}

fn validate_name(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value.contains('\0')
    {
        return Err(VoidCrawlError::Other(format!("invalid {label}: {value:?}")));
    }
    Ok(())
}

fn now_epoch_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default()
}

fn seed_standalone_profile(path: &Path) -> Result<()> {
    let default_dir = path.join("Default");
    fs::create_dir_all(&default_dir)
        .map_err(|e| VoidCrawlError::Other(format!("create {}: {e}", default_dir.display())))?;
    let preferences = default_dir.join("Preferences");
    if !preferences.exists() {
        fs::write(&preferences, "{}")
            .map_err(|e| VoidCrawlError::Other(format!("write {}: {e}", preferences.display())))?;
    }
    Ok(())
}

fn acquire_profile_lock(id: &str, path: &Path) -> Result<ManagedProfileLease> {
    if !path.is_dir() {
        return Err(VoidCrawlError::ProfileNotFound {
            name:     id.to_string(),
            searched: vec![path.display().to_string()],
        });
    }
    let lock_path = path.join(".voidcrawl.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| VoidCrawlError::Other(format!("open {}: {e}", lock_path.display())))?;

    let lock_box: Box<FileLock<File>> = Box::new(FileLock::new(file));
    let lock_ref: &'static mut FileLock<File> = Box::leak(lock_box);
    let guard = match lock_ref.try_write() {
        Ok(guard) => guard,
        Err(e) if e.kind() == ErrorKind::WouldBlock => {
            return Err(profile_busy(id, &lock_path));
        }
        Err(e) => return Err(VoidCrawlError::Other(format!("lock {}: {e}", lock_path.display()))),
    };
    let mut guard = guard;
    write_metadata(&mut guard).map_err(|e| {
        VoidCrawlError::Other(format!("write lease metadata {}: {e}", lock_path.display()))
    })?;
    Ok(ManagedProfileLease { id: id.to_string(), path: path.to_path_buf(), _lock: guard })
}

fn profile_busy(name: &str, lock_path: &Path) -> VoidCrawlError {
    let owner = read_metadata(lock_path);
    VoidCrawlError::ProfileBusy {
        name:        name.to_string(),
        pid:         owner.as_ref().map(|m| m.pid),
        acquired_at: owner.map(|m| m.acquired_at),
    }
}

fn lock_status(path: &Path) -> Result<ProfileStatus> {
    if !path.is_dir() {
        return Ok(ProfileStatus::Missing);
    }
    let lock_path = path.join(".voidcrawl.lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| VoidCrawlError::Other(format!("open {}: {e}", lock_path.display())))?;
    let mut lock = FileLock::new(file);
    match lock.try_write() {
        Ok(_guard) => Ok(ProfileStatus::Available),
        Err(e) if e.kind() == ErrorKind::WouldBlock => Ok(ProfileStatus::Locked),
        Err(e) => Err(VoidCrawlError::Other(format!("lock {}: {e}", lock_path.display()))),
    }
}

fn dir_size(path: &Path) -> Result<u64> {
    let mut total = 0;
    let entries = fs::read_dir(path)
        .map_err(|e| VoidCrawlError::Other(format!("read_dir {}: {e}", path.display())))?;
    for entry in entries {
        let entry = entry.map_err(|e| VoidCrawlError::Other(format!("read_dir entry: {e}")))?;
        let path = entry.path();
        let metadata = entry
            .metadata()
            .map_err(|e| VoidCrawlError::Other(format!("metadata {}: {e}", path.display())))?;
        if metadata.is_dir() {
            total += dir_size(&path)?;
        } else {
            total += metadata.len();
        }
    }
    Ok(total)
}

fn validate_split_copies(copies: usize) -> Result<()> {
    if !(2..=MAX_PROFILE_SPLIT_COPIES).contains(&copies) {
        return Err(VoidCrawlError::Other(format!(
            "profile split copies must be between 2 and {MAX_PROFILE_SPLIT_COPIES}"
        )));
    }
    Ok(())
}

fn copy_regular_file_if_present(source: &Path, destination: &Path) -> Result<()> {
    let Ok(metadata) = source.symlink_metadata() else {
        return Ok(());
    };
    if !metadata.file_type().is_file() {
        return Ok(());
    }
    fs::copy(source, destination).map_err(|e| {
        VoidCrawlError::Other(format!(
            "copy profile metadata {} to {}: {e}",
            source.display(),
            destination.display()
        ))
    })?;
    Ok(())
}

fn copy_snapshot_recursively(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination).map_err(|e| {
        VoidCrawlError::Other(format!("create snapshot dir {}: {e}", destination.display()))
    })?;
    for entry in fs::read_dir(source).map_err(|e| {
        VoidCrawlError::Other(format!("read snapshot source {}: {e}", source.display()))
    })? {
        let entry =
            entry.map_err(|e| VoidCrawlError::Other(format!("read snapshot entry: {e}")))?;
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        if name_text == ".voidcrawl.lock" || name_text.starts_with("Singleton") {
            continue;
        }
        let from = entry.path();
        let to = destination.join(name);
        let file_type = entry.file_type().map_err(|e| {
            VoidCrawlError::Other(format!("read snapshot file type {}: {e}", from.display()))
        })?;
        // Chrome profiles should be self-contained. Never follow arbitrary
        // links out of the leased source tree into a worker snapshot.
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            copy_snapshot_recursively(&from, &to)?;
        } else if file_type.is_file() {
            fs::copy(&from, &to).map_err(|e| {
                VoidCrawlError::Other(format!(
                    "copy snapshot {} to {}: {e}",
                    from.display(),
                    to.display()
                ))
            })?;
        }
    }
    Ok(())
}

fn copy_dir_recursively(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination).map_err(|e| {
        VoidCrawlError::Other(format!("create clone destination {}: {e}", destination.display()))
    })?;
    for entry in fs::read_dir(source)
        .map_err(|e| VoidCrawlError::Other(format!("read_dir {}: {e}", source.display())))?
    {
        let entry = entry.map_err(|e| VoidCrawlError::Other(format!("read_dir entry: {e}")))?;
        let source_path = entry.path();
        if source_path.file_name().and_then(|s| s.to_str()) == Some(".voidcrawl.lock") {
            continue;
        }
        let destination_path = destination.join(entry.file_name());
        let metadata = entry.metadata().map_err(|e| {
            VoidCrawlError::Other(format!("metadata {}: {e}", source_path.display()))
        })?;
        if metadata.is_dir() {
            copy_dir_recursively(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path).map_err(|e| {
                VoidCrawlError::Other(format!(
                    "copy {} to {}: {e}",
                    source_path.display(),
                    destination_path.display()
                ))
            })?;
        }
    }
    seed_standalone_profile(destination)
}
