use crate::tooling::manifest::Native;
use crate::tooling::registry::{NativeSpec, PodVersion};
use crate::tooling::target;
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum InstallError {
    Download(String),
    Checksum(String),
    Extraction(String),
    Io(std::io::Error),
    /// pit has no artifact-naming convention at all for the host platform
    /// (not a supported os/arch combination).
    UnsupportedHost {
        pod: String,
        vers: String,
    },
    /// The host is a target pit knows how to name, but this pod's registry
    /// entry doesn't list an artifact for it.
    NativeUnavailable {
        pod: String,
        vers: String,
        host: String,
        available: Vec<String>,
    },
    /// The registry's artifact hash for this target no longer matches what
    /// `pit.lock` recorded.
    LockMismatch {
        pod: String,
        vers: String,
        target: String,
        locked: String,
        found: String,
    },
    /// `--offline` was requested and the native artifact isn't installed yet.
    Offline {
        pod: String,
        vers: String,
        target: String,
    },
    /// Building the pod's engine from its published sources failed.
    Build(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            InstallError::Download(msg) => write!(f, "download failed: {}", msg),
            InstallError::Checksum(msg) => write!(f, "checksum mismatch: {}", msg),
            InstallError::Extraction(msg) => write!(f, "extraction failed: {}", msg),
            InstallError::Io(e) => write!(f, "I/O error: {}", e),
            InstallError::UnsupportedHost { pod, vers } => write!(
                f,
                "pod '{pod}@{vers}' requires a native library, but pit has no artifact \
                 naming convention for this platform"
            ),
            InstallError::NativeUnavailable {
                pod,
                vers,
                host,
                available,
            } => write!(
                f,
                "pod '{pod}@{vers}' provides no native library for this platform\n\
                 \x20 host target: {host}\n\
                 \x20 available:   {}\n\
                 \x20 this pod cannot be used here until its author adds {host} to its release matrix",
                available.join(", ")
            ),
            InstallError::LockMismatch {
                pod,
                vers,
                target,
                locked,
                found,
            } => write!(
                f,
                "native library for '{pod}@{vers}' does not match pit.lock\n\
                 \x20 target:   {target}\n\
                 \x20 expected: {locked}   (pit.lock)\n\
                 \x20 found:    {found}   (registry)\n\
                 \x20 the registry entry changed after this lockfile was written; run \
                 `pit update {pod}` or restore pit.lock from version control"
            ),
            InstallError::Offline { pod, vers, target } => write!(
                f,
                "offline mode: native library for '{pod}@{vers}' ({target}) is not installed\n\
                 \x20 run without --offline once to fetch it"
            ),
            InstallError::Build(msg) => write!(f, "native build failed: {msg}"),
        }
    }
}
impl std::error::Error for InstallError {}

impl From<std::io::Error> for InstallError {
    fn from(e: std::io::Error) -> Self {
        InstallError::Io(e)
    }
}

const MAX_POD_DOWNLOAD_BYTES: usize = 64 * 1024 * 1024;
const MAX_DECOMPRESSED_POD_BYTES: u64 = 512 * 1024 * 1024;
const MAX_POD_ARCHIVE_ENTRIES: usize = 100_000;
const INSTALL_CHECKSUM_MARKER: &str = ".pit-archive-checksum";

/// Decompresses and extracts a `.pit.zst` archive into `dest_dir`.
///
/// Pods are downloaded from URLs published in registry entries that reach the
/// registry through a reviewed pull request, but the archive itself is untrusted
/// content once it leaves that review: a malicious or buggy publish could still
/// craft an entry that tries to escape `dest_dir` (`..` components), overwrite
/// arbitrary files through a symlink, or plant a `native/` directory to race the
/// separately-verified native artifact download. All three are rejected here
/// before any file is written.
fn extract_pod_archive(compressed_data: &[u8], dest_dir: &Path) -> Result<(), InstallError> {
    fs::create_dir_all(dest_dir)?;
    let destination_root = fs::canonicalize(dest_dir)?;
    let decoder = zstd::stream::read::Decoder::new(compressed_data)
        .map_err(|e| InstallError::Extraction(e.to_string()))?;
    let mut limited = decoder.take(MAX_DECOMPRESSED_POD_BYTES + 1);
    let mut decompressed = Vec::new();
    limited
        .read_to_end(&mut decompressed)
        .map_err(|e| InstallError::Extraction(e.to_string()))?;
    if decompressed.len() as u64 > MAX_DECOMPRESSED_POD_BYTES {
        return Err(InstallError::Extraction(format!(
            "pod archive exceeds the {MAX_DECOMPRESSED_POD_BYTES} byte decompressed limit"
        )));
    }
    let mut archive = tar::Archive::new(decompressed.as_slice());
    let mut archive_root: Option<std::ffi::OsString> = None;
    let mut seen = HashSet::new();
    let mut entry_count = 0usize;

    for entry in archive
        .entries()
        .map_err(|e| InstallError::Extraction(e.to_string()))?
    {
        entry_count += 1;
        if entry_count > MAX_POD_ARCHIVE_ENTRIES {
            return Err(InstallError::Extraction(format!(
                "pod archive exceeds the {MAX_POD_ARCHIVE_ENTRIES} entry limit"
            )));
        }
        let mut entry = entry.map_err(|e| InstallError::Extraction(e.to_string()))?;
        let raw_path = entry
            .path()
            .map_err(|e| InstallError::Extraction(e.to_string()))?
            .into_owned();

        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            return Err(InstallError::Extraction(format!(
                "archive entry '{}' has type {:?}; pods may contain only regular files and directories",
                raw_path.display(),
                kind
            )));
        }

        let mut components = raw_path.components();
        let Some(std::path::Component::Normal(root)) = components.next() else {
            return Err(InstallError::Extraction(format!(
                "archive entry '{}' has no pod root",
                raw_path.display()
            )));
        };
        if !crate::tooling::safe_archive_component(root) {
            return Err(InstallError::Extraction(format!(
                "archive entry '{}' contains an unsafe root component",
                raw_path.display()
            )));
        }
        let root = root.to_os_string();
        match &archive_root {
            Some(expected) if expected != &root => {
                return Err(InstallError::Extraction(format!(
                    "archive contains multiple pod roots: '{}' and '{}'",
                    expected.to_string_lossy(),
                    root.to_string_lossy()
                )));
            }
            None => archive_root = Some(root),
            _ => {}
        }
        if !seen.insert(raw_path.clone()) {
            return Err(InstallError::Extraction(format!(
                "archive contains duplicate entry '{}'",
                raw_path.display()
            )));
        }

        let stripped: PathBuf = components.collect();
        if stripped.as_os_str().is_empty() {
            if !kind.is_dir() {
                return Err(InstallError::Extraction(format!(
                    "archive root '{}' is not a directory",
                    raw_path.display()
                )));
            }
            continue;
        }
        for component in stripped.components() {
            let std::path::Component::Normal(name) = component else {
                return Err(InstallError::Extraction(format!(
                    "archive entry '{}' escapes the pod directory",
                    raw_path.display()
                )));
            };
            if !crate::tooling::safe_archive_component(name) {
                return Err(InstallError::Extraction(format!(
                    "archive entry '{}' contains an unsafe path component",
                    raw_path.display()
                )));
            }
        }
        if stripped
            .components()
            .next()
            .and_then(|component| match component {
                std::path::Component::Normal(name) => name.to_str(),
                _ => None,
            })
            .is_some_and(|name| name.eq_ignore_ascii_case("native"))
        {
            return Err(InstallError::Extraction(
                "archive contains a 'native/' directory; native artifacts are published \
                 as separate release assets and may not be packed into the .pit.zst"
                    .to_string(),
            ));
        }

        let dest = destination_root.join(&stripped);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
            let canonical_parent = fs::canonicalize(parent)?;
            if !canonical_parent.starts_with(&destination_root) {
                return Err(InstallError::Extraction(format!(
                    "archive entry '{}' escapes the pod directory",
                    raw_path.display()
                )));
            }
        }
        if fs::symlink_metadata(&dest)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(InstallError::Extraction(format!(
                "archive entry '{}' targets a symlink",
                raw_path.display()
            )));
        }
        entry.set_preserve_permissions(false);
        entry
            .unpack(&dest)
            .map_err(|e| InstallError::Extraction(e.to_string()))?;
    }
    if archive_root.is_none() || !destination_root.join("pit.toml").is_file() {
        return Err(InstallError::Extraction(
            "pod archive contains no root directory with pit.toml".to_string(),
        ));
    }
    Ok(())
}

/// Downloads `url`, verifying its blake3 hash against `expected_cksum` before
/// returning the body. Shared by the pod archive download and the native
/// artifact download so both go through one verified path.
async fn download_and_verify(
    client: &reqwest::Client,
    url: &str,
    expected_cksum: &str,
) -> Result<Vec<u8>, InstallError> {
    let mut resp = client
        .get(url)
        .header("User-Agent", "pit/0.1.0")
        .send()
        .await
        .map_err(|e| InstallError::Download(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(InstallError::Download(format!(
            "HTTP {} for {url}",
            resp.status()
        )));
    }

    if resp
        .content_length()
        .is_some_and(|size| size > MAX_POD_DOWNLOAD_BYTES as u64)
    {
        return Err(InstallError::Download(format!(
            "artifact for {url} exceeds the {MAX_POD_DOWNLOAD_BYTES} byte limit"
        )));
    }

    let mut hasher = blake3::Hasher::new();
    let mut data = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| InstallError::Download(e.to_string()))?
    {
        if data.len().saturating_add(chunk.len()) > MAX_POD_DOWNLOAD_BYTES {
            return Err(InstallError::Download(format!(
                "artifact for {url} exceeds the {MAX_POD_DOWNLOAD_BYTES} byte limit"
            )));
        }
        hasher.update(&chunk);
        data.extend_from_slice(&chunk);
    }

    let cksum = hasher.finalize().to_hex().to_string();
    if !crate::tooling::registry::checksums_equal(&cksum, expected_cksum) {
        return Err(InstallError::Checksum(format!(
            "expected {expected_cksum}, got {cksum} for {url}"
        )));
    }
    Ok(data)
}

/// Writes `data` into `dir/filename`, via a same-directory temp file and
/// rename so a reader never observes a partially-written artifact. Mode 0644
/// on Unix: a shared object needs to be readable, never executable.
fn write_file_atomic(dir: &Path, filename: &str, data: &[u8]) -> Result<(), InstallError> {
    fs::create_dir_all(dir)?;
    let mut rng = rand::rng();
    use rand::Rng;
    let tmp = dir.join(format!(".tmp-{:x}", rng.next_u64()));
    let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    if let Err(error) = file.write_all(data).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&tmp);
        return Err(InstallError::Io(error));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644))?;
    }
    if let Err(error) = fs::rename(&tmp, dir.join(filename)) {
        let _ = fs::remove_file(&tmp);
        return Err(InstallError::Io(error));
    }
    Ok(())
}

fn installed_archive_matches(dir: &Path, pod: &PodVersion) -> bool {
    fs::read_to_string(dir.join(INSTALL_CHECKSUM_MARKER)).is_ok_and(|checksum| {
        crate::tooling::registry::checksums_equal(checksum.trim(), &pod.cksum)
    }) && dir.join("pit.toml").is_file()
}

fn replace_pod_dir(staged: &Path, destination: &Path) -> Result<(), InstallError> {
    let existing = match fs::symlink_metadata(destination) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(InstallError::Io(error)),
    };
    if !existing {
        return fs::rename(staged, destination).map_err(InstallError::Io);
    }

    let parent = destination
        .parent()
        .ok_or_else(|| InstallError::Extraction("installed pod path has no parent".to_string()))?;
    let mut backup_dir = None;
    for _ in 0..16 {
        let candidate = parent.join(format!(".backup-{:x}", rand::random::<u64>()));
        match fs::create_dir(&candidate) {
            Ok(()) => {
                backup_dir = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(InstallError::Io(error)),
        }
    }
    let backup_dir = backup_dir.ok_or_else(|| {
        InstallError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a unique pod backup directory",
        ))
    })?;
    let backup = backup_dir.join("old");
    if let Err(error) = fs::rename(destination, &backup) {
        let _ = fs::remove_dir(&backup_dir);
        return Err(InstallError::Io(error));
    }
    if let Err(error) = fs::rename(staged, destination) {
        let restore = fs::rename(&backup, destination);
        if restore.is_ok() {
            let _ = fs::remove_dir(&backup_dir);
            return Err(InstallError::Io(error));
        }
        return Err(InstallError::Extraction(format!(
            "could not install replacement pod: {error}; could not restore previous pod from {}: {}",
            backup.display(),
            restore.unwrap_err()
        )));
    }
    if let Err(error) = fs::remove_dir_all(&backup_dir) {
        eprintln!(
            "Installed pod, but could not remove backup {}: {error}",
            backup_dir.display()
        );
    }
    Ok(())
}

/// Downloads this pod's native library (and MSVC import library, if any) for
/// the host platform into `native_dir`, verifying against `locked` first when
/// a lockfile entry is present.
async fn fetch_native_into(
    pod_name: &str,
    pod_vers: &str,
    spec: &NativeSpec,
    native_dir: &Path,
    locked: Option<&BTreeMap<String, String>>,
    locked_implib: Option<&BTreeMap<String, String>>,
    offline: bool,
) -> Result<(), InstallError> {
    let key = target::host().ok_or_else(|| InstallError::UnsupportedHost {
        pod: pod_name.to_string(),
        vers: pod_vers.to_string(),
    })?;
    let artifact = spec
        .artifacts
        .get(key)
        .ok_or_else(|| InstallError::NativeUnavailable {
            pod: pod_name.to_string(),
            vers: pod_vers.to_string(),
            host: key.to_string(),
            available: spec.artifacts.keys().cloned().collect(),
        })?;

    if let Some(locked) = locked
        && let Some(expected) = locked.get(key)
        && !crate::tooling::registry::checksums_equal(expected, &artifact.cksum)
    {
        return Err(InstallError::LockMismatch {
            pod: pod_name.to_string(),
            vers: pod_vers.to_string(),
            target: key.to_string(),
            locked: expected.clone(),
            found: artifact.cksum.clone(),
        });
    }

    if let Some(implib) = &artifact.implib
        && let Some(expected) = locked_implib.and_then(|values| values.get(key))
        && !crate::tooling::registry::checksums_equal(expected, &implib.cksum)
    {
        return Err(InstallError::LockMismatch {
            pod: pod_name.to_string(),
            vers: pod_vers.to_string(),
            target: format!("{key} import library"),
            locked: expected.clone(),
            found: implib.cksum.clone(),
        });
    }

    if offline {
        return Err(InstallError::Offline {
            pod: pod_name.to_string(),
            vers: pod_vers.to_string(),
            target: key.to_string(),
        });
    }

    let client = reqwest::Client::new();
    let data = download_and_verify(&client, &artifact.url, &artifact.cksum).await?;
    let local_name =
        target::local_name(&spec.lib).ok_or_else(|| InstallError::UnsupportedHost {
            pod: pod_name.to_string(),
            vers: pod_vers.to_string(),
        })?;
    write_file_atomic(native_dir, &local_name, &data)?;

    match (&artifact.implib, target::local_implib_name(&spec.lib)) {
        (Some(implib), Some(local_implib)) => {
            let implib_data = download_and_verify(&client, &implib.url, &implib.cksum).await?;
            write_file_atomic(native_dir, &local_implib, &implib_data)?;
        }
        (None, Some(_)) if target::implib_asset_name(&spec.lib, key).is_some() => {
            return Err(InstallError::Extraction(format!(
                "pod '{pod_name}@{pod_vers}' ships a Windows DLL with no import library\n\
                 \x20 the MSVC linker cannot link against a .dll directly; the pod must \
                 publish an import library alongside its DLL for target '{key}'"
            )));
        }
        _ => {}
    }

    Ok(())
}

/// The `[native]` table from an installed pod's own `pit.toml`, if it declares
/// one. This is what makes source builds work: the archive carries the engine
/// sources alongside the Olive code, so the library can be built on the
/// consumer's machine exactly like Cargo build scripts or pip sdists.
fn installed_native_manifest(dir: &Path) -> Option<Native> {
    let content = fs::read_to_string(dir.join("pit.toml")).ok()?;
    let config: crate::tooling::manifest::Config = toml::from_str(&content).ok()?;
    config.native
}

fn source_built_present(dir: &Path, native: &Native) -> bool {
    target::local_name(&native.lib).is_some_and(|n| dir.join("native").join(n).is_file())
}

/// Builds a pod's engine from its published sources into its own `native/`
/// directory. Runs on the consumer's machine at install time, so no pod
/// author CI, no per-platform uploads, and no waiting are ever required to
/// ship a native pod. A no-op when the pod declares no `[native]` table.
async fn ensure_source_built(
    pod: &PodVersion,
    dir: &Path,
    offline: bool,
) -> Result<(), InstallError> {
    let Some(native) = installed_native_manifest(dir) else {
        return Ok(());
    };
    if source_built_present(dir, &native) {
        return Ok(());
    }
    if offline {
        return Err(InstallError::Offline {
            pod: pod.name.clone(),
            vers: pod.vers.clone(),
            target: "source build".to_string(),
        });
    }
    println!(
        "\x1b[1;32m  Building\x1b[0m {}@{} native library from source (one-time, may take a few minutes)",
        pod.name, pod.vers
    );
    let dir_buf = dir.to_path_buf();
    tokio::task::spawn_blocking(move || crate::tooling::native::ensure_built_in(&dir_buf, &native))
        .await
        .map_err(|e| InstallError::Build(format!("task panicked: {e}")))
        .and_then(|r| r.map_err(InstallError::Build))
}

/// Repairs an already-installed pod's native library: a verified registry
/// artifact is re-downloaded when stale, otherwise the engine is built from
/// the published sources. A no-op for pure source pods.
async fn ensure_native_artifact(
    pod: &PodVersion,
    final_dir: &Path,
    locked: Option<&BTreeMap<String, String>>,
    locked_implib: Option<&BTreeMap<String, String>>,
    offline: bool,
) -> Result<(), InstallError> {
    if let Some(spec) = &pod.native
        && let Some(key) = target::host()
        && let Some(artifact) = spec.artifacts.get(key)
        && let Some(expected) = locked.and_then(|values| values.get(key))
        && !crate::tooling::registry::checksums_equal(expected, &artifact.cksum)
    {
        return Err(InstallError::LockMismatch {
            pod: pod.name.clone(),
            vers: pod.vers.clone(),
            target: key.to_string(),
            locked: expected.clone(),
            found: artifact.cksum.clone(),
        });
    }
    if let Some(spec) = &pod.native
        && let Some(key) = target::host()
        && let (Some(artifact), Some(local_name)) =
            (spec.artifacts.get(key), target::local_name(&spec.lib))
    {
        let native_dir = final_dir.join("native");
        let up_to_date = fs::read(native_dir.join(&local_name)).is_ok_and(|data| {
            crate::tooling::registry::checksums_equal(
                blake3::hash(&data).to_hex().as_ref(),
                &artifact.cksum,
            )
        }) && artifact.implib.as_ref().is_none_or(|implib| {
            target::local_implib_name(&spec.lib).is_none_or(|local_implib| {
                fs::read(native_dir.join(&local_implib)).is_ok_and(|data| {
                    crate::tooling::registry::checksums_equal(
                        blake3::hash(&data).to_hex().as_ref(),
                        &implib.cksum,
                    )
                })
            })
        });
        if up_to_date {
            return Ok(());
        }
        return fetch_native_into(
            &pod.name,
            &pod.vers,
            spec,
            &native_dir,
            locked,
            locked_implib,
            offline,
        )
        .await;
    }
    if pod.native.is_some() && target::host().is_none() {
        return Err(InstallError::UnsupportedHost {
            pod: pod.name.clone(),
            vers: pod.vers.clone(),
        });
    }
    if let Some(spec) = &pod.native
        && installed_native_manifest(final_dir).is_none()
    {
        let host = target::host().unwrap_or("unknown");
        return Err(InstallError::NativeUnavailable {
            pod: pod.name.clone(),
            vers: pod.vers.clone(),
            host: host.to_string(),
            available: spec.artifacts.keys().cloned().collect(),
        });
    }
    ensure_source_built(pod, final_dir, offline).await
}

pub async fn install_pod_atomic(
    pod: &PodVersion,
    final_dir: PathBuf,
    locked: Option<&BTreeMap<String, String>>,
    locked_implib: Option<&BTreeMap<String, String>>,
    offline: bool,
) -> Result<(), InstallError> {
    if final_dir.exists() {
        if installed_archive_matches(&final_dir, pod) {
            return ensure_native_artifact(pod, &final_dir, locked, locked_implib, offline).await;
        }
        if offline {
            return Err(InstallError::Checksum(format!(
                "installed pod '{}@{}' has no matching verified archive marker",
                pod.name, pod.vers
            )));
        }
    }
    if offline {
        return Err(InstallError::Offline {
            pod: pod.name.clone(),
            vers: pod.vers.clone(),
            target: "source archive".to_string(),
        });
    }

    println!("\x1b[1;32m  Downloading\x1b[0m {}@{}", pod.name, pod.vers);

    let pods_base = final_dir.parent().unwrap().parent().unwrap();
    let mut rng = rand::rng();
    use rand::Rng;
    fs::create_dir_all(pods_base).map_err(InstallError::Io)?;
    let mut tmp_dir = None;
    for _ in 0..16 {
        let candidate = pods_base.join(format!(".tmp-{:x}", rng.next_u64()));
        match fs::create_dir(&candidate) {
            Ok(()) => {
                tmp_dir = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(InstallError::Io(error)),
        }
    }
    let Some(tmp_dir) = tmp_dir else {
        return Err(InstallError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a unique pod staging directory",
        )));
    };

    let client = reqwest::Client::new();
    let compressed_data = match download_and_verify(&client, &pod.dl, &pod.cksum).await {
        Ok(data) => data,
        Err(e) => {
            let _ = fs::remove_dir_all(&tmp_dir);
            return Err(e);
        }
    };

    let tmp_dir_clone = tmp_dir.clone();
    let extraction: Result<(), InstallError> =
        tokio::task::spawn_blocking(move || extract_pod_archive(&compressed_data, &tmp_dir_clone))
            .await
            .map_err(|e| InstallError::Extraction(format!("task panicked: {}", e)))
            .and_then(|r| r);
    if let Err(e) = extraction {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(e);
    }
    let parsed_config = fs::read_to_string(tmp_dir.join("pit.toml"))
        .map_err(|error| InstallError::Extraction(format!("invalid extracted pit.toml: {error}")))
        .and_then(|content| {
            toml::from_str::<crate::tooling::manifest::Config>(&content).map_err(|error| {
                InstallError::Extraction(format!("invalid extracted pit.toml: {error}"))
            })
        });
    let parsed_config = match parsed_config {
        Ok(config) => config,
        Err(error) => {
            let _ = fs::remove_dir_all(&tmp_dir);
            return Err(error);
        }
    };
    let Some(manifest_pod) = &parsed_config.pod else {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(InstallError::Extraction(
            "archive manifest has no [pod] table".to_string(),
        ));
    };
    if manifest_pod.name != pod.name {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(InstallError::Extraction(format!(
            "archive manifest declares pod '{}', expected '{}'",
            manifest_pod.name, pod.name
        )));
    }
    if manifest_pod.version != pod.vers {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(InstallError::Extraction(format!(
            "archive manifest declares version '{}', expected '{}'",
            manifest_pod.version, pod.vers
        )));
    }
    let extracted_root =
        fs::canonicalize(&tmp_dir).map_err(|error| InstallError::Extraction(error.to_string()))?;
    if let Err(error) =
        crate::tooling::manifest::validate_pod_files(&parsed_config, &extracted_root)
    {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(InstallError::Extraction(error));
    }

    let has_host_artifact = pod.native.as_ref().is_some_and(|spec| {
        target::host().is_some_and(|key| {
            spec.artifacts.contains_key(key) && target::local_name(&spec.lib).is_some()
        })
    });
    if has_host_artifact {
        let spec = pod.native.as_ref().expect("checked above");
        let native_dir = tmp_dir.join("native");
        if let Err(e) = fetch_native_into(
            &pod.name,
            &pod.vers,
            spec,
            &native_dir,
            locked,
            locked_implib,
            offline,
        )
        .await
        {
            let _ = fs::remove_dir_all(&tmp_dir);
            return Err(e);
        }
    } else if installed_native_manifest(&tmp_dir).is_some() {
        println!(
            "\x1b[1;32m  Building\x1b[0m {}@{} native library from source (one-time, may take a few minutes)",
            pod.name, pod.vers
        );
        let build_dir = tmp_dir.clone();
        let build_result = tokio::task::spawn_blocking(move || {
            let native = installed_native_manifest(&build_dir)
                .ok_or_else(|| "pod stopped declaring [native] mid-install".to_string())?;
            crate::tooling::native::ensure_built_in(&build_dir, &native)
        })
        .await
        .map_err(|e| InstallError::Build(format!("task panicked: {e}")))
        .and_then(|r| r.map_err(InstallError::Build));
        if let Err(e) = build_result {
            let _ = fs::remove_dir_all(&tmp_dir);
            return Err(e);
        }
    }

    if let Err(error) = fs::write(
        tmp_dir.join(INSTALL_CHECKSUM_MARKER),
        format!("{}\n", pod.cksum),
    ) {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(InstallError::Io(error));
    }
    if let Some(parent) = final_dir.parent() {
        fs::create_dir_all(parent)?;
    }

    if let Err(error) = replace_pod_dir(&tmp_dir, &final_dir) {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(error);
    }
    println!("\x1b[1;32m  Installed\x1b[0m {}@{}", pod.name, pod.vers);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tooling::registry::PodVersion;

    #[test]
    fn failed_repair_keeps_previous_installation() {
        let base = std::env::temp_dir().join(format!(
            "olive-install-repair-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let final_dir = base.join("pods").join("test").join("1.0.0");
        fs::create_dir_all(&final_dir).unwrap();
        fs::write(final_dir.join("pit.toml"), "old installation").unwrap();
        let pod = PodVersion {
            name: "test".to_string(),
            vers: "1.0.0".to_string(),
            deps: vec![],
            cksum: blake3::hash(b"new archive").to_hex().to_string(),
            dl: "not-a-url".to_string(),
            yanked: false,
            olive_req: None,
            native: None,
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        assert!(
            runtime
                .block_on(install_pod_atomic(
                    &pod,
                    final_dir.clone(),
                    None,
                    None,
                    false
                ))
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(final_dir.join("pit.toml")).unwrap(),
            "old installation"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn replacement_swaps_complete_directory_and_cleans_backup() {
        let base = std::env::temp_dir().join(format!(
            "olive-install-swap-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let parent = base.join("test");
        let final_dir = parent.join("1.0.0");
        let staged = base.join("staged");
        fs::create_dir_all(&final_dir).unwrap();
        fs::create_dir_all(&staged).unwrap();
        fs::write(final_dir.join("old"), b"old").unwrap();
        fs::write(staged.join("new"), b"new").unwrap();
        replace_pod_dir(&staged, &final_dir).unwrap();
        assert!(final_dir.join("new").is_file());
        assert!(!final_dir.join("old").exists());
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 1);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn failed_swap_restores_previous_directory() {
        let base = std::env::temp_dir().join(format!(
            "olive-install-rollback-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let final_dir = base.join("test").join("1.0.0");
        fs::create_dir_all(&final_dir).unwrap();
        fs::write(final_dir.join("old"), b"old").unwrap();
        assert!(replace_pod_dir(&base.join("missing"), &final_dir).is_err());
        assert_eq!(fs::read(final_dir.join("old")).unwrap(), b"old");
        assert_eq!(
            fs::read_dir(final_dir.parent().unwrap()).unwrap().count(),
            1
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn install_error_download_display() {
        let e = InstallError::Download("timeout".to_string());
        assert_eq!(format!("{e}"), "download failed: timeout");
    }

    #[test]
    fn install_error_checksum_display() {
        let e = InstallError::Checksum("expected abc, got def".to_string());
        assert_eq!(format!("{e}"), "checksum mismatch: expected abc, got def");
    }

    #[test]
    fn install_error_extraction_display() {
        let e = InstallError::Extraction("corrupt archive".to_string());
        assert_eq!(format!("{e}"), "extraction failed: corrupt archive");
    }

    #[test]
    fn install_error_io_display() {
        let inner = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let e = InstallError::Io(inner);
        assert!(format!("{e}").contains("I/O error"));
    }

    #[test]
    fn install_error_from_io() {
        let inner = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let e: InstallError = inner.into();
        assert!(matches!(e, InstallError::Io(_)));
    }

    #[test]
    fn install_error_impl_std_error() {
        fn assert_error<E: std::error::Error>() {}
        assert_error::<InstallError>();
    }

    #[cfg(unix)]
    #[test]
    fn install_pod_atomic_builds_native_from_published_sources() {
        use std::io::{Read, Write};

        let _lock = crate::commands::utils::CWD_LOCK.lock().unwrap();
        let base = std::env::temp_dir().join("olive_install_src_build");
        let _ = fs::remove_dir_all(&base);
        let author = base.join("author");
        fs::create_dir_all(author.join("src")).unwrap();
        let built = target::built_name("mini");
        let local = target::local_name("mini").unwrap();
        fs::write(
            author.join("pit.toml"),
            format!(
                "[pod]\nname = \"mini\"\nversion = \"1.0.0\"\nentry = \"src/lib.liv\"\n\n[dependencies]\n\n[native]\nlib = \"mini\"\nbuild = [\"sh\", \"-c\", \"mkdir -p target/release && printf fake > target/release/{built}\"]\n"
            ),
        )
        .unwrap();
        fs::write(author.join("src").join("lib.liv"), "fn f():\n    pass\n").unwrap();

        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            for (disk, tar_path) in [
                (author.join("pit.toml"), "mini-1.0.0/pit.toml"),
                (author.join("src").join("lib.liv"), "mini-1.0.0/src/lib.liv"),
            ] {
                let bytes = fs::read(&disk).unwrap();
                let mut header = tar::Header::new_gnu();
                header.set_size(bytes.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder
                    .append_data(&mut header, tar_path, bytes.as_slice())
                    .unwrap();
            }
            builder.finish().unwrap();
        }
        let compressed = zstd::encode_all(tar_bytes.as_slice(), 3).unwrap();
        let cksum = blake3::hash(&compressed).to_hex().to_string();

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = compressed.clone();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = vec![0u8; 4096];
                let _ = stream.read(&mut buf);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });

        let pod = PodVersion {
            name: "mini".to_string(),
            vers: "1.0.0".to_string(),
            deps: vec![],
            cksum,
            dl: format!("http://127.0.0.1:{port}/mini.pit.zst"),
            yanked: false,
            olive_req: None,
            native: None,
        };
        let final_dir = base.join("pods").join("mini").join("1.0.0");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(install_pod_atomic(
            &pod,
            final_dir.clone(),
            None,
            None,
            false,
        ));
        assert!(result.is_ok(), "install failed: {:?}", result.err());
        assert_eq!(
            fs::read(final_dir.join("native").join(&local)).unwrap(),
            b"fake"
        );
        assert!(final_dir.join("src").join("lib.liv").is_file());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn install_pod_atomic_accepts_existing_dir_only_with_matching_marker() {
        let dir = std::env::temp_dir().join("olive_install_test_exists");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("pit.toml"),
            "[pod]\nname = \"test\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let digest = blake3::hash(b"verified").to_hex().to_string();
        fs::write(dir.join(INSTALL_CHECKSUM_MARKER), format!("{digest}\n")).unwrap();

        let pod = PodVersion {
            name: "test".to_string(),
            vers: "1.0.0".to_string(),
            deps: vec![],
            cksum: digest.to_uppercase(),
            dl: String::new(),
            yanked: false,
            olive_req: None,
            native: None,
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(install_pod_atomic(&pod, dir.clone(), None, None, true));
        assert!(result.is_ok(), "{result:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    fn native_pod(name: &str, vers: &str, spec: NativeSpec) -> PodVersion {
        PodVersion {
            name: name.to_string(),
            vers: vers.to_string(),
            deps: vec![],
            cksum: String::new(),
            dl: String::new(),
            yanked: false,
            olive_req: None,
            native: Some(spec),
        }
    }

    fn fake_artifact(cksum: &str) -> crate::tooling::registry::NativeArtifact {
        crate::tooling::registry::NativeArtifact {
            file: "libfake.so".to_string(),
            url: "https://example.invalid/libfake.so".to_string(),
            cksum: cksum.to_string(),
            implib: None,
        }
    }

    #[test]
    fn ensure_native_artifact_is_a_noop_for_pods_without_native() {
        let dir = std::env::temp_dir().join("olive_native_test_noop");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let pod = PodVersion {
            name: "term".to_string(),
            vers: "0.1.5".to_string(),
            deps: vec![],
            cksum: String::new(),
            dl: String::new(),
            yanked: false,
            olive_req: None,
            native: None,
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(ensure_native_artifact(&pod, &dir, None, None, false));
        assert!(result.is_ok());
        assert!(!dir.join("native").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_pod_atomic_errors_when_host_target_has_no_artifact() {
        let dir = std::env::temp_dir().join("olive_native_test_unavailable_dir_missing");
        let _ = fs::remove_dir_all(&dir);

        let mut artifacts = BTreeMap::new();
        artifacts.insert("nonexistent-target".to_string(), fake_artifact("abc"));
        let pod = native_pod(
            "tokenizer",
            "0.3.0",
            NativeSpec {
                lib: "tokenizer".to_string(),
                artifacts,
            },
        );

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(ensure_native_artifact(&pod, &dir, None, None, false));
        match result {
            Err(InstallError::NativeUnavailable { available, .. }) => {
                assert_eq!(available, vec!["nonexistent-target".to_string()]);
            }
            other => panic!("expected NativeUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn ensure_native_artifact_reports_lock_mismatch_before_downloading() {
        let Some(host) = target::host() else {
            return;
        };
        let dir = std::env::temp_dir().join("olive_native_test_lock_mismatch");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let mut artifacts = BTreeMap::new();
        artifacts.insert(host.to_string(), fake_artifact("registry-hash"));
        let pod = native_pod(
            "tokenizer",
            "0.3.0",
            NativeSpec {
                lib: "tokenizer".to_string(),
                artifacts,
            },
        );

        let mut locked = BTreeMap::new();
        locked.insert(host.to_string(), "locked-hash".to_string());

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(ensure_native_artifact(
            &pod,
            &dir,
            Some(&locked),
            None,
            false,
        ));
        match result {
            Err(InstallError::LockMismatch {
                locked: l, found, ..
            }) => {
                assert_eq!(l, "locked-hash");
                assert_eq!(found, "registry-hash");
            }
            other => panic!("expected LockMismatch, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_native_artifact_reports_offline_before_downloading() {
        let Some(host) = target::host() else {
            return;
        };
        let dir = std::env::temp_dir().join("olive_native_test_offline");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let mut artifacts = BTreeMap::new();
        artifacts.insert(host.to_string(), fake_artifact("abc"));
        let pod = native_pod(
            "tokenizer",
            "0.3.0",
            NativeSpec {
                lib: "tokenizer".to_string(),
                artifacts,
            },
        );

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(ensure_native_artifact(&pod, &dir, None, None, true));
        assert!(matches!(result, Err(InstallError::Offline { .. })));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_native_artifact_skips_download_when_installed_hash_matches() {
        let Some(host) = target::host() else {
            return;
        };
        let dir = std::env::temp_dir().join("olive_native_test_up_to_date");
        let _ = fs::remove_dir_all(&dir);
        let native_dir = dir.join("native");
        fs::create_dir_all(&native_dir).unwrap();

        let data = b"pretend shared object contents";
        let cksum = blake3::hash(data).to_hex().to_string();
        let local_name = target::local_name("fake").unwrap();
        fs::write(native_dir.join(&local_name), data).unwrap();

        let mut artifacts = BTreeMap::new();
        artifacts.insert(host.to_string(), fake_artifact(&cksum.to_uppercase()));
        let pod = native_pod(
            "fake",
            "1.0.0",
            NativeSpec {
                lib: "fake".to_string(),
                artifacts,
            },
        );

        // offline: true would error if a download were attempted, so a
        // successful Ok(()) here proves the up-to-date check short-circuits
        // before ever reaching fetch_native_into.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(ensure_native_artifact(&pod, &dir, None, None, true));
        assert!(result.is_ok(), "{result:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    fn build_test_archive(entries: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            for (path, kind, data) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(*kind);
                header.set_path(path).unwrap();
                header.set_size(data.len() as u64);
                header.set_mode(if kind.is_dir() { 0o755 } else { 0o644 });
                header.set_cksum();
                builder.append(&header, *data).unwrap();
            }
            builder.finish().unwrap();
        }
        zstd::encode_all(tar_bytes.as_slice(), 3).unwrap()
    }

    /// `tar::Header::set_path` refuses to encode a `..` component at all, so a
    /// traversal attempt has to be built by writing the raw fixed-size `name`
    /// field directly, exactly as a hand-crafted malicious archive would.
    fn archive_with_raw_name(raw_name: &str, data: &[u8]) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_old();
            header.set_entry_type(tar::EntryType::Regular);
            let name_field = &mut header.as_old_mut().name;
            let bytes = raw_name.as_bytes();
            name_field[..bytes.len()].copy_from_slice(bytes);
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, data).unwrap();
            builder.finish().unwrap();
        }
        zstd::encode_all(tar_bytes.as_slice(), 3).unwrap()
    }

    #[test]
    fn extract_pod_archive_rejects_parent_dir_traversal() {
        let archive = archive_with_raw_name("pkg-1.0/../../../../etc/evil", b"malicious");
        let dest = std::env::temp_dir().join("olive_extract_test_traversal");
        let _ = fs::remove_dir_all(&dest);
        let result = extract_pod_archive(&archive, &dest);
        assert!(matches!(result, Err(InstallError::Extraction(_))));
        assert!(!dest.parent().unwrap().join("etc").exists());
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn extract_pod_archive_rejects_hardlink_entries() {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Link);
            header.set_path("pkg-1.0/src/evil-hardlink").unwrap();
            header.set_link_name("/etc/passwd").unwrap();
            header.set_size(0);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, std::io::empty()).unwrap();
            builder.finish().unwrap();
        }
        let archive = zstd::encode_all(tar_bytes.as_slice(), 3).unwrap();
        let dest = std::env::temp_dir().join("olive_extract_test_hardlink");
        let _ = fs::remove_dir_all(&dest);
        let result = extract_pod_archive(&archive, &dest);
        assert!(matches!(result, Err(InstallError::Extraction(_))));
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn extract_pod_archive_rejects_symlink_entries() {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_path("pkg-1.0/src/evil-link").unwrap();
            header.set_link_name("/etc/passwd").unwrap();
            header.set_size(0);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, std::io::empty()).unwrap();
            builder.finish().unwrap();
        }
        let archive = zstd::encode_all(tar_bytes.as_slice(), 3).unwrap();
        let dest = std::env::temp_dir().join("olive_extract_test_symlink");
        let _ = fs::remove_dir_all(&dest);
        let result = extract_pod_archive(&archive, &dest);
        assert!(matches!(result, Err(InstallError::Extraction(_))));
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn extract_pod_archive_rejects_native_directory() {
        let archive = build_test_archive(&[(
            "pkg-1.0/native/libtokenizer.so",
            tar::EntryType::Regular,
            b"not a real library",
        )]);
        let dest = std::env::temp_dir().join("olive_extract_test_native");
        let _ = fs::remove_dir_all(&dest);
        let result = extract_pod_archive(&archive, &dest);
        assert!(matches!(result, Err(InstallError::Extraction(msg)) if msg.contains("native/")));
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn extract_pod_archive_accepts_well_formed_pod() {
        let archive = build_test_archive(&[
            ("pkg-1.0/pit.toml", tar::EntryType::Regular, b"[pod]\n"),
            ("pkg-1.0/src", tar::EntryType::Directory, b""),
            (
                "pkg-1.0/src/lib.liv",
                tar::EntryType::Regular,
                b"fn main():\n    pass\n",
            ),
        ]);
        let dest = std::env::temp_dir().join("olive_extract_test_wellformed");
        let _ = fs::remove_dir_all(&dest);
        let result = extract_pod_archive(&archive, &dest);
        assert!(result.is_ok(), "{:?}", result);
        assert!(dest.join("pit.toml").is_file());
        assert!(dest.join("src/lib.liv").is_file());
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn extract_pod_archive_rejects_multiple_roots_and_duplicates() {
        let dest = std::env::temp_dir().join("olive_extract_roots");
        let _ = fs::remove_dir_all(&dest);
        fs::create_dir_all(&dest).unwrap();

        let multiple = build_test_archive(&[
            (
                "one/pit.toml",
                tar::EntryType::Regular,
                b"[pod]\nname = \"one\"\n",
            ),
            (
                "two/pit.toml",
                tar::EntryType::Regular,
                b"[pod]\nname = \"two\"\n",
            ),
        ]);
        assert!(extract_pod_archive(&multiple, &dest).is_err());

        let duplicate = build_test_archive(&[
            (
                "one/pit.toml",
                tar::EntryType::Regular,
                b"[pod]\nname = \"one\"\n",
            ),
            (
                "one/pit.toml",
                tar::EntryType::Regular,
                b"[pod]\nname = \"one\"\n",
            ),
        ]);
        assert!(extract_pod_archive(&duplicate, &dest).is_err());
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn extract_pod_archive_rejects_case_alias_of_native_directory() {
        let archive = build_test_archive(&[
            (
                "mini/pit.toml",
                tar::EntryType::Regular,
                b"[pod]\nname = \"mini\"\n",
            ),
            ("mini/Native/libmini.so", tar::EntryType::Regular, b"bad"),
        ]);
        let dest = std::env::temp_dir().join("olive_extract_native_case");
        let _ = fs::remove_dir_all(&dest);
        fs::create_dir_all(&dest).unwrap();
        assert!(extract_pod_archive(&archive, &dest).is_err());
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn install_rejects_unmarked_directory_and_missing_offline_archive() {
        let existing = std::env::temp_dir().join("olive_install_unmarked");
        let _ = fs::remove_dir_all(&existing);
        fs::create_dir_all(&existing).unwrap();
        fs::write(
            existing.join("pit.toml"),
            "[pod]\nname = \"x\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let pod = PodVersion {
            name: "x".to_string(),
            vers: "1.0.0".to_string(),
            deps: vec![],
            cksum: "expected".to_string(),
            dl: "https://invalid.example/archive".to_string(),
            yanked: false,
            olive_req: None,
            native: None,
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let marked = runtime.block_on(install_pod_atomic(&pod, existing.clone(), None, None, true));
        assert!(matches!(marked, Err(InstallError::Checksum(_))));

        let missing = std::env::temp_dir().join("olive_install_missing_offline");
        let _ = fs::remove_dir_all(&missing);
        let offline = runtime.block_on(install_pod_atomic(&pod, missing, None, None, true));
        assert!(matches!(offline, Err(InstallError::Offline { .. })));
        let _ = fs::remove_dir_all(&existing);
    }
}
