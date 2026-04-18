use crate::tooling::registry::PodVersion;
use std::fs;
use std::path::PathBuf;

#[derive(Debug)]
pub enum InstallError {
    Download(String),
    Checksum(String),
    Extraction(String),
    Io(std::io::Error),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            InstallError::Download(msg) => write!(f, "download failed: {}", msg),
            InstallError::Checksum(msg) => write!(f, "checksum mismatch: {}", msg),
            InstallError::Extraction(msg) => write!(f, "extraction failed: {}", msg),
            InstallError::Io(e) => write!(f, "I/O error: {}", e),
        }
    }
}
impl std::error::Error for InstallError {}

impl From<std::io::Error> for InstallError {
    fn from(e: std::io::Error) -> Self {
        InstallError::Io(e)
    }
}

pub async fn install_pod_atomic(pod: &PodVersion, final_dir: PathBuf) -> Result<(), InstallError> {
    if final_dir.exists() {
        return Ok(());
    }

    println!("\x1b[1;32m  Downloading\x1b[0m {}@{}", pod.name, pod.vers);

    let pods_base = final_dir.parent().unwrap().parent().unwrap();
    let mut rng = rand::rng();
    use rand::Rng;
    let tmp_dir = pods_base.join(format!(".tmp-{:x}", rng.next_u64()));
    fs::create_dir_all(&tmp_dir)?;

    let client = reqwest::Client::new();
    let mut resp = client
        .get(&pod.dl)
        .header("User-Agent", "pit/0.1.0")
        .send()
        .await
        .map_err(|e| InstallError::Download(e.to_string()))?;

    if !resp.status().is_success() {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(InstallError::Download(format!("HTTP {}", resp.status())));
    }

    let mut hasher = blake3::Hasher::new();
    let mut compressed_data = Vec::new();

    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| InstallError::Download(e.to_string()))?
    {
        hasher.update(&chunk);
        compressed_data.extend_from_slice(&chunk);
    }

    let cksum = hasher.finalize().to_hex().to_string();
    if cksum != pod.cksum {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(InstallError::Checksum(format!(
            "expected {}, got {}",
            pod.cksum, cksum
        )));
    }

    let tmp_dir_clone = tmp_dir.clone();

    tokio::task::spawn_blocking(move || -> Result<(), InstallError> {
        let decompressed = zstd::decode_all(compressed_data.as_slice())
            .map_err(|e| InstallError::Extraction(e.to_string()))?;
        let mut archive = tar::Archive::new(decompressed.as_slice());

        for entry in archive
            .entries()
            .map_err(|e| InstallError::Extraction(e.to_string()))?
        {
            let mut entry = entry.map_err(|e| InstallError::Extraction(e.to_string()))?;
            let raw_path = entry
                .path()
                .map_err(|e| InstallError::Extraction(e.to_string()))?;
            let stripped: PathBuf = raw_path.components().skip(1).collect();
            if stripped.as_os_str().is_empty() {
                continue;
            }
            let dest = tmp_dir_clone.join(&stripped);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            entry
                .unpack(&dest)
                .map_err(|e| InstallError::Extraction(e.to_string()))?;
        }
        Ok(())
    })
    .await
    .map_err(|e| InstallError::Extraction(format!("task panicked: {}", e)))??;

    if let Some(parent) = final_dir.parent() {
        fs::create_dir_all(parent)?;
    }

    match fs::rename(&tmp_dir, &final_dir) {
        Ok(_) => {
            println!("\x1b[1;32m  Installed\x1b[0m {}@{}", pod.name, pod.vers);
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&tmp_dir);
            if !final_dir.exists() {
                return Err(InstallError::Io(e));
            }
        }
    }

    Ok(())
}
