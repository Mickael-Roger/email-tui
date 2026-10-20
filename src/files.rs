use std::path::{Path, PathBuf};

pub fn icon(content_type: &str) -> &'static str {
    match content_type {
        "application/pdf" => "📄",
        "application/zip" | "application/gzip" | "application/x-tar" => "▣",
        kind if kind.starts_with("image/") => "🖼",
        kind if kind.starts_with("audio/") => "♫",
        kind if kind.starts_with("video/") => "▶",
        kind if kind.starts_with("text/") => "≡",
        _ => "📎",
    }
}

pub fn download_directory() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| ".".into());
    if let Some(path) = std::env::var_os("XDG_DOWNLOAD_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return path;
    }
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    if let Ok(dirs) = std::fs::read_to_string(config.join("user-dirs.dirs")) {
        for line in dirs.lines() {
            if let Some(value) = line.trim().strip_prefix("XDG_DOWNLOAD_DIR=")
                && let Ok(words) = shell_words::split(value)
                && let [value] = words.as_slice()
            {
                let path = if value == "$HOME" || value == "${HOME}" {
                    home.clone()
                } else if let Some(path) = value
                    .strip_prefix("$HOME/")
                    .or_else(|| value.strip_prefix("${HOME}/"))
                {
                    home.join(path)
                } else {
                    PathBuf::from(value)
                };
                if path.is_absolute() {
                    return path;
                }
            }
        }
    }
    home.join("Downloads")
}

pub fn destination(value: &str) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(!value.trim().is_empty(), "Choose a destination directory");
    if value == "~" || value.starts_with("~/") {
        let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
        return Ok(PathBuf::from(home).join(value.strip_prefix("~/").unwrap_or("")));
    }
    Ok(value.into())
}

/// Keep server-provided filenames inside the chosen directory, without replacing files.
pub fn save_attachments(
    directory: &Path,
    attachments: &[crate::mime::Attachment],
) -> anyhow::Result<Vec<PathBuf>> {
    use anyhow::Context;
    use std::{fs::OpenOptions, io::Write};
    std::fs::create_dir_all(directory)
        .with_context(|| format!("Create {}", directory.display()))?;
    let directory = directory.canonicalize()?;
    let mut saved = Vec::new();
    let result = (|| -> anyhow::Result<()> {
        for (index, attachment) in attachments.iter().enumerate() {
            let name = attachment.name.rsplit(['/', '\\']).next().unwrap_or("");
            let mut name: String = name
                .chars()
                .filter(|c| {
                    !c.is_control()
                        && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
                })
                .collect();
            while name.len() > 200 {
                name.pop();
            }
            if name.trim().is_empty() || matches!(name.as_str(), "." | "..") {
                name = format!("attachment-{}", index + 1);
            }
            let path = Path::new(&name);
            let stem = path
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or(&name);
            let extension = path.extension().and_then(|name| name.to_str());
            let mut suffix = 0usize;
            let (path, mut file) = loop {
                let name = if suffix == 0 {
                    name.clone()
                } else {
                    format!(
                        "{stem} ({suffix}){}",
                        extension
                            .map(|extension| format!(".{extension}"))
                            .unwrap_or_default()
                    )
                };
                let path = directory.join(name);
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                match options.open(&path) {
                    Ok(file) => break (path, file),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => suffix += 1,
                    Err(error) => {
                        return Err(error).with_context(|| format!("Save {}", path.display()));
                    }
                }
            };
            saved.push(path.clone());
            file.write_all(&attachment.bytes)
                .and_then(|_| file.sync_all())
                .with_context(|| format!("Write {}", path.display()))?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        // Only roll back files created by this batch, never pre-existing files.
        for path in saved {
            let _ = std::fs::remove_file(path);
        }
        return Err(error);
    }
    Ok(saved)
}

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
    pub size: u64,
}

pub fn browse(path: &Path) -> anyhow::Result<Vec<FileEntry>> {
    let mut entries = vec![];
    if let Some(parent) = path.parent() {
        entries.push(FileEntry {
            path: parent.into(),
            name: "..".into(),
            directory: true,
            size: 0,
        });
    }
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let Ok(metadata) = std::fs::metadata(entry.path()) else {
            continue;
        };
        if metadata.is_dir() || metadata.is_file() {
            entries.push(FileEntry {
                path: entry.path(),
                name: entry.file_name().to_string_lossy().into_owned(),
                directory: metadata.is_dir(),
                size: metadata.len(),
            });
        }
    }
    entries.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

pub fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "txt" | "log" | "md" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "eml" => "message/rfc822",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_preserve_bytes_sanitize_names_and_never_follow_existing_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let downloads = directory.path().join("downloads");
        std::fs::create_dir(&downloads).unwrap();
        let existing = directory.path().join("existing.bin");
        std::fs::write(&existing, b"original").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&existing, downloads.join("report.bin")).unwrap();
        #[cfg(not(unix))]
        std::fs::write(downloads.join("report.bin"), b"original").unwrap();
        let attachments: Vec<_> = [
            "../../report.bin",
            "C:\\outside\\report.bin",
            "../..",
            "été 日本語.bin",
        ]
        .into_iter()
        .map(|name| crate::mime::Attachment {
            name: name.into(),
            content_type: "application/octet-stream".into(),
            bytes: std::sync::Arc::from([0u8, 255, 1, 42]),
        })
        .collect();
        let saved = save_attachments(&downloads, &attachments).unwrap();
        assert_eq!(saved.len(), 4);
        assert_eq!(saved[0].file_name().unwrap(), "report (1).bin");
        assert_eq!(saved[1].file_name().unwrap(), "report (2).bin");
        assert_eq!(saved[2].file_name().unwrap(), "attachment-3");
        for path in saved {
            assert_eq!(path.parent().unwrap(), downloads.canonicalize().unwrap());
            assert_eq!(std::fs::read(&path).unwrap(), [0, 255, 1, 42]);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
        assert_eq!(std::fs::read(existing).unwrap(), b"original");
    }
}
