//! GitHub sources are fetched by Rust, never by a shell or browser-side Git client.
use buglogin_domain::entities::GitExtensionSource;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Read},
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_DOWNLOAD: usize = 64 * 1024 * 1024;
const MAX_EXPANDED: u64 = 256 * 1024 * 1024;

pub struct GitPackage {
    pub source: GitExtensionSource,
    pub revision: String,
    pub bytes: Vec<u8>,
    pub manifest: Value,
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("BugLogin-Extension-Updater")
        .timeout(Duration::from_secs(12))
        .connect_timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())
}

async fn download(url: &str) -> Result<Vec<u8>, String> {
    use futures_util::StreamExt;
    let response = client()?
        .get(url)
        .send()
        .await
        .map_err(|_| "Không kết nối được GitHub.")?;
    if !response.status().is_success() {
        return Err(format!(
            "GitHub trả về HTTP {}.",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_DOWNLOAD as u64)
    {
        return Err("Gói tiện ích vượt quá 64 MiB.".into());
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "Tải tiện ích GitHub bị gián đoạn.")?;
        if bytes.len().saturating_add(chunk.len()) > MAX_DOWNLOAD {
            return Err("Gói tiện ích vượt quá 64 MiB.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub async fn source_from_url(input: &str) -> Result<GitExtensionSource, String> {
    let mut source = GitExtensionSource {
        repository_url: input
            .trim()
            .trim_end_matches('/')
            .trim_end_matches(".git")
            .into(),
        reference: "main".into(),
        subdirectory: String::new(),
    };
    source.validate().map_err(str::to_owned)?;
    let path = url::Url::parse(&source.repository_url)
        .map_err(|e| e.to_string())?
        .path()
        .to_owned();
    let meta: Value =
        serde_json::from_slice(&download(&format!("https://api.github.com/repos{path}")).await?)
            .map_err(|_| "Không đọc được thông tin repository.")?;
    source.reference = meta
        .get("default_branch")
        .and_then(Value::as_str)
        .ok_or("Repository chưa có nhánh mặc định.")?
        .to_owned();
    source.validate().map_err(str::to_owned)?;
    Ok(source)
}

pub async fn resolve_revision(source: &GitExtensionSource) -> Result<String, String> {
    source.validate().map_err(str::to_owned)?;
    let repository = url::Url::parse(&source.repository_url).map_err(|e| e.to_string())?;
    let mut url = url::Url::parse(&format!(
        "https://api.github.com/repos{}/commits/",
        repository.path()
    ))
    .map_err(|e| e.to_string())?;
    url.path_segments_mut()
        .map_err(|_| "Repository URL không hợp lệ.")?
        .pop_if_empty()
        .push(&source.reference);
    let commit: Value = serde_json::from_slice(&download(url.as_str()).await?)
        .map_err(|_| "Không đọc được commit GitHub.")?;
    commit
        .get("sha")
        .and_then(Value::as_str)
        .filter(|sha| sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_owned)
        .ok_or_else(|| "GitHub không trả về commit hợp lệ.".into())
}

pub async fn fetch(source: GitExtensionSource, revision: String) -> Result<GitPackage, String> {
    source.validate().map_err(str::to_owned)?;
    if revision.len() != 40 || !revision.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Commit Git không hợp lệ.".into());
    }
    let repo = url::Url::parse(&source.repository_url).map_err(|e| e.to_string())?;
    let bytes = download(&format!(
        "https://codeload.github.com{}/zip/{revision}",
        repo.path()
    ))
    .await?;
    let normalized = normalize_repository_zip(&bytes, &source.subdirectory)?;
    let manifest = read_manifest(&normalized)?;
    Ok(GitPackage {
        source,
        revision,
        bytes: normalized,
        manifest,
    })
}

pub fn source_id(source: &GitExtensionSource) -> String {
    source.extension_id()
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn safe_entry(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && !name.contains('\\')
        && !name.contains(':')
        && !name.chars().any(char::is_control)
        && name
            .split('/')
            .all(|part| part != ".." && part != "." && !part.ends_with(' ') && !part.ends_with('.'))
}

pub fn normalize_repository_zip(bytes: &[u8], subdirectory: &str) -> Result<Vec<u8>, String> {
    let mut zip =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| "Gói GitHub ZIP không hợp lệ.")?;
    if zip.len() > 10_000 {
        return Err("Gói tiện ích có quá nhiều tệp.".into());
    }
    let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut total = 0u64;
    let mut seen = std::collections::BTreeSet::new();
    let mut root: Option<String> = None;
    for index in 0..zip.len() {
        let file = zip
            .by_index(index)
            .map_err(|_| "Không đọc được ZIP entry.")?;
        let entry_name = file
            .name()
            .map_err(|_| "Không đọc được tên ZIP entry.")?
            .into_owned();
        if !safe_entry(&entry_name)
            || file
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err("Gói tiện ích chứa đường dẫn hoặc liên kết không an toàn.".into());
        }
        let (prefix, relative) = entry_name
            .split_once('/')
            .ok_or("ZIP repository thiếu thư mục gốc.")?;
        if root.as_ref().is_some_and(|root| root != prefix) {
            return Err("ZIP repository có nhiều thư mục gốc.".into());
        }
        root.get_or_insert_with(|| prefix.to_owned());
        let relative = if subdirectory.is_empty() {
            relative
        } else {
            let Some(relative) =
                relative.strip_prefix(&format!("{}/", subdirectory.trim_end_matches('/')))
            else {
                continue;
            };
            relative
        }
        .to_owned();
        if relative.is_empty() || file.is_dir() {
            continue;
        }
        total = total
            .checked_add(file.size())
            .ok_or("ZIP vượt giới hạn giải nén.")?;
        if total > MAX_EXPANDED || !seen.insert(relative.clone()) {
            return Err("ZIP vượt giới hạn hoặc trùng đường dẫn.".into());
        }
        output
            .start_file(
                relative,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .map_err(|e| e.to_string())?;
        let copied = std::io::copy(&mut file.take(MAX_EXPANDED + 1), &mut output)
            .map_err(|e| e.to_string())?;
        if copied > MAX_EXPANDED {
            return Err("ZIP vượt giới hạn giải nén.".into());
        }
    }
    Ok(output.finish().map_err(|e| e.to_string())?.into_inner())
}

pub fn read_manifest(bytes: &[u8]) -> Result<Value, String> {
    let bytes = archive_payload(bytes)?;
    let mut zip =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| "Tệp ZIP tiện ích không hợp lệ.")?;
    let manifest = zip
        .by_name("manifest.json")
        .map_err(|_| "Gói thiếu manifest.json ở thư mục gốc.")?;
    if manifest.size() > 1024 * 1024 {
        return Err("manifest.json vượt giới hạn.".into());
    }
    let value: Value =
        serde_json::from_reader(manifest).map_err(|_| "manifest.json không hợp lệ.")?;
    if value.get("name").and_then(Value::as_str).is_none()
        || value.get("version").and_then(Value::as_str).is_none()
        || !matches!(
            value.get("manifest_version").and_then(Value::as_u64),
            Some(2 | 3)
        )
    {
        return Err("Manifest thiếu tên, phiên bản hoặc manifest_version hợp lệ.".into());
    }
    Ok(value)
}

fn archive_payload(bytes: &[u8]) -> Result<&[u8], String> {
    if bytes.starts_with(b"PK") {
        return Ok(bytes);
    }
    if bytes.starts_with(b"Cr24") && bytes.get(4..8) == Some(&3u32.to_le_bytes()) {
        let length = bytes
            .get(8..12)
            .and_then(|value| value.try_into().ok())
            .map(u32::from_le_bytes)
            .ok_or("CRX header không hợp lệ.")?;
        return bytes
            .get(
                12usize
                    .checked_add(length as usize)
                    .ok_or("CRX header không hợp lệ.")?..,
            )
            .filter(|payload| payload.starts_with(b"PK"))
            .ok_or_else(|| "CRX payload không hợp lệ.".into());
    }
    Err("Chỉ hỗ trợ ZIP hoặc CRX3.".into())
}

fn directory(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            Err("Đường dẫn cài tiện ích không an toàn.".into())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(path).map_err(|e| e.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Replace only this profile's unpacked copy. Its stable path preserves Chromium extension ID/storage.
pub fn install(
    root: &Path,
    profile_id: &str,
    extension_id: &str,
    revision: &str,
    bytes: &[u8],
) -> Result<PathBuf, String> {
    read_manifest(bytes)?;
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    directory(root)?;
    let profile_root = root.join(hash(profile_id.as_bytes()));
    directory(&profile_root)?;
    let target = profile_root.join(hash(extension_id.as_bytes()));
    let staging = target.with_extension("staging");
    let backup = target.with_extension("previous");
    if !target.exists() && backup.exists() {
        std::fs::rename(&backup, &target).map_err(|e| e.to_string())?;
    }
    for leftover in [&staging, &backup] {
        if leftover.exists() {
            directory(leftover)?;
            std::fs::remove_dir_all(leftover).map_err(|e| e.to_string())?;
        }
    }
    directory(&staging)?;
    let mut zip =
        zip::ZipArchive::new(Cursor::new(archive_payload(bytes)?)).map_err(|e| e.to_string())?;
    if zip.len() > 10_000 {
        return Err("Gói tiện ích có quá nhiều tệp.".into());
    }
    let mut total = 0u64;
    let mut seen = std::collections::BTreeSet::new();
    for index in 0..zip.len() {
        let entry = zip.by_index(index).map_err(|e| e.to_string())?;
        let entry_name = entry.name().map_err(|e| e.to_string())?.into_owned();
        if !safe_entry(&entry_name)
            || entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err("Gói tiện ích chứa đường dẫn không an toàn.".into());
        }
        if entry.is_dir() {
            continue;
        }
        total = total
            .checked_add(entry.size())
            .ok_or("ZIP vượt giới hạn.")?;
        if total > MAX_EXPANDED || !seen.insert(entry_name.clone()) {
            return Err("ZIP vượt giới hạn hoặc trùng đường dẫn.".into());
        }
        let output = staging.join(&entry_name);
        std::fs::create_dir_all(output.parent().ok_or("ZIP path không hợp lệ.")?)
            .map_err(|e| e.to_string())?;
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(output)
            .map_err(|e| e.to_string())?;
        let copied = std::io::copy(&mut entry.take(MAX_EXPANDED + 1), &mut file)
            .map_err(|e| e.to_string())?;
        if copied > MAX_EXPANDED {
            return Err("ZIP vượt giới hạn giải nén.".into());
        }
        file.sync_all().map_err(|e| e.to_string())?;
    }
    std::fs::write(
        staging.join(".buglogin-extension.json"),
        json!({"revision":revision}).to_string(),
    )
    .map_err(|e| e.to_string())?;
    if target.exists() {
        directory(&target)?;
        std::fs::rename(&target, &backup).map_err(|e| e.to_string())?;
    }
    if let Err(error) = std::fs::rename(&staging, &target) {
        if backup.exists() {
            let _ = std::fs::rename(&backup, &target);
        }
        return Err(error.to_string());
    }
    if backup.exists() {
        directory(&backup)?;
        std::fs::remove_dir_all(&backup).map_err(|e| e.to_string())?;
    }
    Ok(target)
}

pub fn installed(root: &Path, profile_id: &str, extension_id: &str) -> Option<(PathBuf, String)> {
    let path = root
        .join(hash(profile_id.as_bytes()))
        .join(hash(extension_id.as_bytes()));
    if std::fs::symlink_metadata(&path)
        .ok()?
        .file_type()
        .is_symlink()
    {
        return None;
    }
    let value: Value =
        serde_json::from_slice(&std::fs::read(path.join(".buglogin-extension.json")).ok()?).ok()?;
    if !path.join("manifest.json").is_file() {
        return None;
    }
    Some((path, value.get("revision")?.as_str()?.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn package(name: &str, body: &[u8]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(body).unwrap();
        writer.finish().unwrap().into_inner()
    }
    const MANIFEST: &[u8] = br#"{"name":"Test","version":"1.0","manifest_version":3}"#;
    #[test]
    fn git_repository_root_is_stripped_and_manifest_validated() {
        let normalized =
            normalize_repository_zip(&package("repo-main/manifest.json", MANIFEST), "").unwrap();
        assert_eq!(read_manifest(&normalized).unwrap()["name"], "Test");
        assert!(normalize_repository_zip(&package("repo-main/../escape", b"bad"), "").is_err());
        assert!(normalize_repository_zip(&package("repo-main/a\\escape", b"bad"), "").is_err());
    }
    #[test]
    fn install_updates_in_place_and_retains_previous_on_invalid_archive() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-extension-test-{}-{}",
            std::process::id(),
            crate::remote_api::sha256_hex(format!("{:?}", std::time::SystemTime::now()).as_bytes())
        ));
        let first = install(
            &root,
            "profile-a",
            "extension-a",
            "commit-1",
            &package("manifest.json", MANIFEST),
        )
        .unwrap();
        let second = install(
            &root,
            "profile-a",
            "extension-a",
            "commit-2",
            &package("manifest.json", MANIFEST),
        )
        .unwrap();
        assert_eq!(first, second);
        assert!(
            install(
                &root,
                "profile-a",
                "extension-a",
                "bad",
                &package("manifest.json", b"bad")
            )
            .is_err()
        );
        assert_eq!(
            installed(&root, "profile-a", "extension-a").unwrap().1,
            "commit-2"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
