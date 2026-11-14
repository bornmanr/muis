//! Snor model catalog and downloader.
//!
//! Weights are never bundled. The settings dialog lists three presets
//! and downloads one only when the user asks, verifying its sha256
//! before the file is used. Downloads resume from a `.part` file.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub file: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
    pub license: &'static str,
}

/// Apache-2.0 Q4_K_M GGUF presets from the plan's three tiers.
pub const CATALOG: &[Preset] = &[
    Preset {
        id: "low",
        label: "Qwen2.5-1.5B-Instruct (low, CPU-friendly)",
        file: "qwen2.5-1.5b-instruct-q4_k_m.gguf",
        url: "https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/main/qwen2.5-1.5b-instruct-q4_k_m.gguf",
        sha256: "6a1a2eb6d15622bf3c96857206351ba97e1af16c30d7a74ee38970e434e9407e",
        size: 1_117_320_736,
        license: "Apache-2.0",
    },
    Preset {
        id: "medium",
        label: "Qwen3-4B-Instruct-2507 (medium, quality/speed)",
        file: "Qwen_Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
        url: "https://huggingface.co/bartowski/Qwen_Qwen3-4B-Instruct-2507-GGUF/resolve/main/Qwen_Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
        sha256: "2fde00ce69dd4899c70d020845e2638353015bba0fdf161b3eb965f2bca4464e",
        size: 2_497_280_736,
        license: "Apache-2.0",
    },
    Preset {
        id: "high",
        label: "Qwen3-8B (high, hard errors, opt-in)",
        file: "Qwen3-8B-Q4_K_M.gguf",
        url: "https://huggingface.co/Qwen/Qwen3-8B-GGUF/resolve/main/Qwen3-8B-Q4_K_M.gguf",
        sha256: "d98cdcbd03e17ce47681435b5150e34c1417f50b5c0019dd560e4882c5745785",
        size: 5_027_783_488,
        license: "Apache-2.0",
    },
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    pub file: String,
    pub size: u64,
    pub sha256: String,
    pub license: String,
    pub installed: bool,
    pub partial: u64,
    pub path: String,
}

pub fn find(id: &str) -> Option<&'static Preset> {
    CATALOG.iter().find(|p| p.id == id)
}

pub fn model_file(dir: &Path, preset: &Preset) -> PathBuf {
    dir.join(preset.file)
}

pub fn part_file(dir: &Path, preset: &Preset) -> PathBuf {
    dir.join(format!("{}.part", preset.file))
}

pub fn info(dir: &Path, preset: &Preset) -> ModelInfo {
    let final_path = model_file(dir, preset);
    let part = part_file(dir, preset);
    ModelInfo {
        id: preset.id.into(),
        label: preset.label.into(),
        file: preset.file.into(),
        size: preset.size,
        sha256: preset.sha256.into(),
        license: preset.license.into(),
        installed: final_path.is_file(),
        partial: std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0),
        path: final_path.to_string_lossy().into_owned(),
    }
}

pub fn list(dir: &Path) -> Vec<ModelInfo> {
    CATALOG.iter().map(|preset| info(dir, preset)).collect()
}

/// Download (or resume) one preset, verifying its sha256 before the
/// `.part` file is moved into place. Progress is byte-based.
pub fn download(
    dir: &Path,
    preset: &Preset,
    mut progress: impl FnMut(u64, u64),
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create model directory: {e}"))?;
    let final_path = model_file(dir, preset);
    if final_path.is_file() {
        return Ok(final_path);
    }
    let part = part_file(dir, preset);
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > preset.size {
        let _ = std::fs::remove_file(&part);
        have = 0;
    }

    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let mut request = client.get(preset.url);
    if have > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let resp = request.send().map_err(|e| format!("download failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("download returned {status}"));
    }
    // A server that ignores Range sends 200 and the whole file again.
    let append = status == reqwest::StatusCode::PARTIAL_CONTENT && have > 0;
    if !append {
        have = 0;
    }
    let total = resp.content_length().map(|len| len + have).unwrap_or(preset.size);

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(&part)
        .map_err(|e| format!("cannot write {}: {e}", part.display()))?;
    let mut reader = resp;
    let mut buffer = [0u8; 64 * 1024];
    let mut received = have;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|e| format!("download interrupted: {e}"))?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|e| format!("cannot write model: {e}"))?;
        received += read as u64;
        progress(received, total);
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);

    if let Err(e) = verify(&part, preset.sha256) {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    std::fs::rename(&part, &final_path).map_err(|e| format!("cannot finish download: {e}"))?;
    Ok(final_path)
}

/// sha256 of a file as lowercase hex.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn verify(path: &Path, expected: &str) -> Result<(), String> {
    let actual = sha256_file(path)?;
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!("checksum mismatch: got {actual}"))
    }
}

pub fn delete(dir: &Path, preset: &Preset) -> Result<(), String> {
    for path in [model_file(dir, preset), part_file(dir, preset)] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot delete {}: {e}", path.display())),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::net::TcpListener;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "muis-snor-models-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn preset(url: &'static str, sha256: &'static str, size: u64) -> Preset {
        Preset {
            id: "test",
            label: "Test",
            file: "test.gguf",
            url,
            sha256,
            size,
            license: "Apache-2.0",
        }
    }

    fn sha256_of(bytes: &[u8]) -> String {
        hex(&Sha256::digest(bytes))
    }

    /// Serve `body` once on a loopback port, ignoring Range.
    fn serve_once(body: &'static [u8]) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut scratch = [0u8; 4096];
            let _ = sock.read(&mut scratch);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            sock.write_all(response.as_bytes()).unwrap();
            sock.write_all(body).unwrap();
            let _ = sock.flush();
        });
        port
    }

    #[test]
    fn catalog_entries_are_unique_and_https() {
        let mut ids = std::collections::HashSet::new();
        for preset in CATALOG {
            assert!(ids.insert(preset.id));
            assert!(preset.url.starts_with("https://"));
            assert_eq!(preset.sha256.len(), 64);
            assert!(preset.size > 0);
        }
    }

    #[test]
    fn downloads_verifies_and_finishes() {
        let dir = temp_dir("ok");
        static BODY: &[u8] = b"hello model";
        let port = serve_once(BODY);
        let url: &'static str = Box::leak(format!("http://127.0.0.1:{port}/m").into_boxed_str());
        let sha: &'static str = Box::leak(sha256_of(BODY).into_boxed_str());
        let preset = preset(url, sha, BODY.len() as u64);

        let mut seen = (0u64, 0u64);
        let path = download(&dir, &preset, |got, total| seen = (got, total)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), BODY);
        assert_eq!(seen.0, BODY.len() as u64);
        assert!(info(&dir, &preset).installed);
        delete(&dir, &preset).unwrap();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_checksum_removes_the_partial_file() {
        let dir = temp_dir("bad");
        static BODY: &[u8] = b"tampered";
        let port = serve_once(BODY);
        let url: &'static str = Box::leak(format!("http://127.0.0.1:{port}/m").into_boxed_str());
        let preset = preset(url, "0000000000000000000000000000000000000000000000000000000000000000", 8);
        let err = download(&dir, &preset, |_, _| {}).unwrap_err();
        assert!(err.contains("checksum mismatch"));
        assert!(!model_file(&dir, &preset).exists());
        assert!(!part_file(&dir, &preset).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_reports_mismatches() {
        let dir = temp_dir("verify");
        let file = dir.join("x");
        std::fs::write(&file, b"abc").unwrap();
        assert!(verify(&file, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad").is_ok());
        assert!(verify(&file, "deadbeef").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_oversized_partial_file_restarts_cleanly() {
        let dir = temp_dir("bigpart");
        static BODY: &[u8] = b"abc";
        let port = serve_once(BODY);
        let url: &'static str = Box::leak(format!("http://127.0.0.1:{port}/m").into_boxed_str());
        let sha: &'static str = Box::leak(sha256_of(BODY).into_boxed_str());
        let preset = preset(url, sha, BODY.len() as u64);
        std::fs::write(part_file(&dir, &preset), vec![0u8; 64]).unwrap();
        let path = download(&dir, &preset, |_, _| {}).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), BODY);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
