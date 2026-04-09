use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::env;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::Duration;
use walkdir::WalkDir;

const MANAGED_DIRS: &[&str] = &["assets", "bin", "sounds"];
const SKIP_DIRS: &[&str] = &[
    ".git",
    "cache",
    "characterdata",
    "crashdump",
    "log",
    "minimap",
    "screenshots",
    "storeimages",
];

#[derive(Debug)]
struct Args {
    source: PathBuf,
    output: PathBuf,
    version: String,
}

#[derive(Debug, Clone, Serialize)]
struct PackageManifest {
    version: String,
    files: Vec<PackageFile>,
}

#[derive(Debug, Clone, Serialize)]
struct PackageFile {
    url: String,
    localfile: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    packedhash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    packedsize: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unpackedhash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unpackedsize: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unpack: Option<bool>,
    #[serde(skip_serializing_if = "is_false")]
    bootstrap_only: bool,
}

#[derive(Debug, Clone, Serialize)]
struct AssetManifest {
    version: String,
    tracked_files: Vec<TrackedFile>,
}

#[derive(Debug, Clone, Serialize)]
struct TrackedFile {
    path: String,
    sha256: String,
    size: u64,
    managed_by_launcher: bool,
    bootstrap_only: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn main() -> Result<()> {
    let args = parse_args()?;
    if args.source == args.output {
        bail!("source and output must be different directories");
    }

    if !args.source.is_dir() {
        bail!("source directory does not exist: {}", args.source.display());
    }

    prepare_output_directory(&args.output)?;

    let files = collect_source_files(&args.source)?;
    let mut package_files = Vec::with_capacity(files.len());
    let mut tracked_files = Vec::with_capacity(files.len());

    for relative_path in files {
        let source_path = args.source.join(&relative_path);
        let relative_str = normalize_relative_path(&relative_path)?;
        let top_level = top_level_dir(&relative_path)?;
        let bootstrap_only = false;

        if top_level == "bin" {
            let output_relative = format!("{relative_str}.lzma");
            let output_path = args.output.join(relative_path_with_extension(&relative_path, "lzma"));
            if let Some(parent) = output_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }

            compress_lzma(&source_path, &output_path)?;
            let packed_hash = hash_file(&output_path)?;
            let packed_size = file_len(&output_path)?;
            let unpacked_hash = hash_file(&source_path)?;
            let unpacked_size = file_len(&source_path)?;

            package_files.push(PackageFile {
                url: output_relative,
                localfile: relative_str.clone(),
                packedhash: Some(packed_hash),
                packedsize: Some(packed_size),
                unpackedhash: Some(unpacked_hash.clone()),
                unpackedsize: Some(unpacked_size),
                unpack: None,
                bootstrap_only,
            });

            tracked_files.push(TrackedFile {
                path: relative_str,
                sha256: unpacked_hash,
                size: unpacked_size,
                managed_by_launcher: true,
                bootstrap_only,
            });
        } else {
            let output_path = args.output.join(&relative_path);
            if let Some(parent) = output_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            fs::copy(&source_path, &output_path).with_context(|| {
                format!(
                    "failed to copy {} -> {}",
                    source_path.display(),
                    output_path.display()
                )
            })?;

            let hash = hash_file(&source_path)?;
            let size = file_len(&source_path)?;

            package_files.push(PackageFile {
                url: relative_str.clone(),
                localfile: relative_str.clone(),
                packedhash: Some(hash.clone()),
                packedsize: Some(size),
                unpackedhash: None,
                unpackedsize: None,
                unpack: Some(false),
                bootstrap_only,
            });

            tracked_files.push(TrackedFile {
                path: relative_str,
                sha256: hash,
                size,
                managed_by_launcher: true,
                bootstrap_only,
            });
        }
    }

    package_files.sort_by(|left, right| left.localfile.cmp(&right.localfile));
    tracked_files.sort_by(|left, right| left.path.cmp(&right.path));

    let resolved_version = if args.version == "auto" {
        auto_version(&tracked_files)
    } else {
        args.version.clone()
    };

    let package = PackageManifest {
        version: resolved_version.clone(),
        files: package_files,
    };
    let assets = AssetManifest {
        version: resolved_version.clone(),
        tracked_files,
    };

    let package_json =
        serde_json::to_string_pretty(&package).context("failed to serialize package manifest")?;
    let assets_json =
        serde_json::to_string_pretty(&assets).context("failed to serialize asset manifest")?;
    let assets_sha256 = sha256_bytes(assets_json.as_bytes());

    fs::write(args.output.join("package.json"), format!("{package_json}\n"))
        .context("failed to write package.json")?;
    fs::write(
        args.output.join("package.json.version"),
        format!("{}\n", resolved_version),
    )
    .context("failed to write package.json.version")?;
    fs::write(args.output.join("assets.json"), format!("{assets_json}\n"))
        .context("failed to write assets.json")?;
    fs::write(
        args.output.join("assets.json.sha256"),
        format!("{}\n", assets_sha256),
    )
    .context("failed to write assets.json.sha256")?;
    fs::write(args.output.join(".gitignore"), public_repo_gitignore())
        .context("failed to write .gitignore")?;
    fs::write(args.output.join(".gitattributes"), public_repo_gitattributes())
        .context("failed to write .gitattributes")?;
    fs::write(args.output.join("README.md"), public_repo_readme(&resolved_version))
        .context("failed to write README.md")?;

    println!(
        "Generated public feed at {} with {} tracked files.",
        args.output.display(),
        package.files.len()
    );

    Ok(())
}

fn auto_version(tracked_files: &[TrackedFile]) -> String {
    let mut hasher = Sha256::new();
    for file in tracked_files {
        hasher.update(file.path.as_bytes());
        hasher.update([0]);
        hasher.update(file.sha256.as_bytes());
        hasher.update([0]);
        hasher.update(file.size.to_le_bytes());
        hasher.update([if file.bootstrap_only { 1 } else { 0 }]);
    }

    let digest = hex_string(&hasher.finalize());
    format!("15.23-prod-{}", &digest[..12])
}

fn prepare_output_directory(output: &Path) -> Result<()> {
    fs::create_dir_all(output)
        .with_context(|| format!("failed to create {}", output.display()))?;

    for dir_name in MANAGED_DIRS.iter().copied().chain(["conf"]) {
        let target = output.join(dir_name);
        if target.exists() {
            remove_dir_all_retry(&target)?;
        }
    }

    for file_name in [
        "package.json",
        "package.json.version",
        "assets.json",
        "assets.json.sha256",
        ".gitignore",
        ".gitattributes",
        "README.md",
    ] {
        let target = output.join(file_name);
        if target.exists() {
            remove_file_retry(&target)?;
        }
    }

    Ok(())
}

fn remove_dir_all_retry(path: &Path) -> Result<()> {
    let mut last_error = None;
    for _ in 0..5 {
        match fs::remove_dir_all(path) {
            Ok(_) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(250));
            }
        }
    }

    Err(last_error
        .with_context(|| format!("failed to clean {}", path.display()))?
        .into())
}

fn remove_file_retry(path: &Path) -> Result<()> {
    let mut last_error = None;
    for _ in 0..5 {
        match fs::remove_file(path) {
            Ok(_) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                thread::sleep(Duration::from_millis(100));
            }
        }
    }

    Err(last_error
        .with_context(|| format!("failed to clean {}", path.display()))?
        .into())
}

fn parse_args() -> Result<Args> {
    let mut source = None;
    let mut output = None;
    let mut version = None;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--source" => source = args.next().map(PathBuf::from),
            "--output" => output = args.next().map(PathBuf::from),
            "--version" => version = args.next(),
            _ => bail!("unknown argument: {arg}"),
        }
    }

    Ok(Args {
        source: source.context("--source is required")?,
        output: output.context("--output is required")?,
        version: version.context("--version is required")?,
    })
}

fn collect_source_files(source: &Path) -> Result<Vec<PathBuf>> {
    let allowed_dirs: BTreeSet<&str> = MANAGED_DIRS.iter().copied().collect();
    let skip_dirs: BTreeSet<&str> = SKIP_DIRS.iter().copied().collect();
    let mut files = Vec::new();

    for entry in WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 {
                return true;
            }

            let relative = match entry.path().strip_prefix(source) {
                Ok(value) => value,
                Err(_) => return false,
            };

            let mut components = relative.components();
            let first = match components.next() {
                Some(Component::Normal(value)) => value.to_string_lossy(),
                _ => return false,
            };

            if skip_dirs.contains(first.as_ref()) {
                return false;
            }

            allowed_dirs.contains(first.as_ref())
        })
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }

        let relative = entry
            .path()
            .strip_prefix(source)
            .with_context(|| format!("failed to relativize {}", entry.path().display()))?;
        files.push(relative.to_path_buf());
    }

    files.sort();
    Ok(files)
}

fn top_level_dir(path: &Path) -> Result<String> {
    match path.components().next() {
        Some(Component::Normal(value)) => Ok(value.to_string_lossy().into_owned()),
        _ => bail!("invalid relative path: {}", path.display()),
    }
}

fn normalize_relative_path(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_string_lossy().into_owned()),
            _ => bail!("unsupported relative path component in {}", path.display()),
        }
    }
    Ok(parts.join("/"))
}

fn relative_path_with_extension(path: &Path, extension: &str) -> PathBuf {
    let mut file_name = path
        .file_name()
        .map(|value| value.to_os_string())
        .unwrap_or_default();
    file_name.push(format!(".{extension}"));
    path.with_file_name(file_name)
}

fn compress_lzma(source: &Path, destination: &Path) -> Result<()> {
    let input = File::open(source)
        .with_context(|| format!("failed to open {}", source.display()))?;
    let output = File::create(destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;
    let mut reader = BufReader::new(input);
    let mut writer = BufWriter::new(output);
    lzma_rs::lzma_compress(&mut reader, &mut writer)
        .with_context(|| format!("failed to compress {}", source.display()))?;
    writer.flush()?;
    Ok(())
}

fn file_len(path: &Path) -> Result<u64> {
    Ok(fs::metadata(path)
        .with_context(|| format!("failed to stat {}", path.display()))?
        .len())
}

fn hash_file(path: &Path) -> Result<String> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(hex_string(&hasher.finalize()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_string(&hasher.finalize())
}

fn hex_string(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(hex_nibble(byte >> 4));
        output.push(hex_nibble(byte & 0x0f));
    }
    output
}

fn hex_nibble(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'a' + (value - 10)) as char,
        _ => unreachable!(),
    }
}

fn public_repo_gitignore() -> String {
    "# Public client feed artifacts are generated intentionally.\n# Runtime folders stay out of the feed.\n/cache/\n/characterdata/\n/crashdump/\n/log/\n/minimap/\n/screenshots/\n/storeimages/\n"
        .to_string()
}

fn public_repo_gitattributes() -> String {
    "* -text\n".to_string()
}

fn public_repo_readme(version: &str) -> String {
    format!(
        "# Penultima Client\n\nPublic update feed for the Penultima Launcher.\n\n- Version: `{version}`\n- Managed folders: `assets`, `bin`, `sounds`\n\nPlayers should use the Penultima Launcher to download and update the client automatically.\n"
    )
}
