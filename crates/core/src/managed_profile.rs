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
    profile::expand_tilde,
};

const MANIFEST_FILE: &str = "registry.json";

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
                return Err(VoidCrawlError::ProfileBusy { name: id.to_string() });
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
                    Err(VoidCrawlError::ProfileBusy { name }) => {
                        last_busy = Some(name);
                    }
                    Err(err) => return Err(err),
                }
            }
            Err(VoidCrawlError::ProfileBusy { name: last_busy.unwrap_or_else(|| name.to_string()) })
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
            return Err(VoidCrawlError::ProfileBusy { name: id.to_string() });
        }
        Err(e) => return Err(VoidCrawlError::Other(format!("lock {}: {e}", lock_path.display()))),
    };
    Ok(ManagedProfileLease { id: id.to_string(), path: path.to_path_buf(), _lock: guard })
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
