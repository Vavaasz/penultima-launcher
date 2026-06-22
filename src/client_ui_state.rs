use anyhow::{Context, Result};
use log::info;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const VAULT_DIR: &str = "client-ui-state";
const LATEST_DIR: &str = "latest";
const SIDEBAR_TEMPLATE_DIR: &str = "sidebar-layout";
const SIDEBAR_TEMPLATE_FILE: &str = "sidebars.json";
const SIDEBAR_TEMPLATE_META_FILE: &str = "meta.json";
const MAX_DISCOVERY_DEPTH: usize = 2;
const MAX_CHARACTERDATA_DEPTH: usize = 4;
const FUTURE_CHARACTERDATA_SLOTS: u32 = 256;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClientUiStateStats {
    pub files: usize,
    pub bytes: u64,
}

#[derive(Clone, Debug)]
struct StateFile {
    relative_path: PathBuf,
    length: u64,
    modified: Option<SystemTime>,
}

#[derive(Clone, Debug)]
struct SidebarLayout {
    value: Value,
    score: u64,
    modified: Option<SystemTime>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CopyMode {
    SnapshotMerge,
    RestoreMissingOnly,
    RestoreOverwriteAll,
}

pub fn ensure_client_ui_state(state_path: &Path, game_path: &Path) -> Result<ClientUiStateStats> {
    fs::create_dir_all(state_path)
        .with_context(|| format!("failed to create {}", state_path.display()))?;

    let vault_path = latest_vault_path(state_path);
    let mut active_stats = summarize_state(game_path)?;
    let vault_stats = summarize_state(&vault_path)?;

    if should_restore_vault(&vault_stats, &active_stats) {
        let restored = restore_client_ui_state(state_path, game_path)?;
        info!(
            "Client UI state restored from vault: {} file(s), {} byte(s)",
            restored.files, restored.bytes
        );
        active_stats = summarize_state(game_path)?;
    } else if vault_stats.files > 0 {
        restore_missing_client_ui_state(state_path, game_path)?;
        active_stats = summarize_state(game_path)?;
    }

    if should_run_discovery(&vault_stats, &active_stats) {
        if let Some(source) = discover_best_state_source(state_path, game_path)? {
            info!(
                "Client UI state discovered automatically from {}",
                source.display()
            );
            copy_state_files(&source, game_path, CopyMode::RestoreOverwriteAll)?;
            active_stats = summarize_state(game_path)?;
        }
    }

    if active_stats.files > 0 {
        sync_sidebar_layout_state(state_path, game_path)?;
        snapshot_client_ui_state(state_path, game_path)?;
    }

    summarize_state(game_path)
}

pub fn snapshot_client_ui_state(state_path: &Path, game_path: &Path) -> Result<ClientUiStateStats> {
    let stats = summarize_state(game_path)?;
    if stats.files == 0 {
        return Ok(stats);
    }

    let vault_path = latest_vault_path(state_path);
    fs::create_dir_all(&vault_path)
        .with_context(|| format!("failed to create {}", vault_path.display()))?;
    let copied = copy_state_files(game_path, &vault_path, CopyMode::SnapshotMerge)?;
    remember_sidebar_layout_template(state_path, game_path)?;
    Ok(copied)
}

pub fn restore_client_ui_state(state_path: &Path, game_path: &Path) -> Result<ClientUiStateStats> {
    let vault_path = latest_vault_path(state_path);
    let vault_stats = summarize_state(&vault_path)?;
    if vault_stats.files == 0 {
        return Ok(ClientUiStateStats::default());
    }

    let active_stats = summarize_state(game_path)?;
    let mode = if should_restore_vault(&vault_stats, &active_stats) {
        CopyMode::RestoreOverwriteAll
    } else {
        CopyMode::RestoreMissingOnly
    };

    let copied = copy_state_files(&vault_path, game_path, mode)?;
    sync_sidebar_layout_state(state_path, game_path)?;
    Ok(copied)
}

fn restore_missing_client_ui_state(
    state_path: &Path,
    game_path: &Path,
) -> Result<ClientUiStateStats> {
    let vault_path = latest_vault_path(state_path);
    copy_state_files(&vault_path, game_path, CopyMode::RestoreMissingOnly)
}

fn latest_vault_path(state_path: &Path) -> PathBuf {
    state_path.join(VAULT_DIR).join(LATEST_DIR)
}

fn sidebar_template_path(vault_path: &Path) -> PathBuf {
    vault_path
        .join(SIDEBAR_TEMPLATE_DIR)
        .join(SIDEBAR_TEMPLATE_FILE)
}

fn sidebar_template_meta_path(vault_path: &Path) -> PathBuf {
    vault_path
        .join(SIDEBAR_TEMPLATE_DIR)
        .join(SIDEBAR_TEMPLATE_META_FILE)
}

fn sync_sidebar_layout_state(state_path: &Path, game_path: &Path) -> Result<()> {
    let vault_path = latest_vault_path(state_path);
    let layout = best_sidebar_layout([
        load_sidebar_layout_template(&vault_path)?,
        find_best_sidebar_layout(&vault_path)?,
        find_best_sidebar_layout(game_path)?,
    ]);

    if let Some(layout) = layout {
        save_sidebar_layout_template(&vault_path, &layout.value)?;
        apply_sidebar_layout_template(state_path, game_path, &layout.value)?;
    }

    Ok(())
}

fn remember_sidebar_layout_template(state_path: &Path, game_path: &Path) -> Result<()> {
    let vault_path = latest_vault_path(state_path);
    let layout = best_sidebar_layout([
        load_sidebar_layout_template(&vault_path)?,
        find_best_sidebar_layout(game_path)?,
        find_best_sidebar_layout(&vault_path)?,
    ]);

    if let Some(layout) = layout {
        save_sidebar_layout_template(&vault_path, &layout.value)?;
    }

    Ok(())
}

fn load_sidebar_layout_template(vault_path: &Path) -> Result<Option<SidebarLayout>> {
    let path = sidebar_template_path(vault_path);
    read_sidebar_layout(&path)
}

fn save_sidebar_layout_template(vault_path: &Path, value: &Value) -> Result<()> {
    let path = sidebar_template_path(vault_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    write_json_pretty(&path, value)
}

fn find_best_sidebar_layout(root: &Path) -> Result<Option<SidebarLayout>> {
    let characterdata = root.join("characterdata");
    if !characterdata.is_dir() {
        return Ok(None);
    }

    let mut best = None;
    for entry in fs::read_dir(&characterdata)
        .with_context(|| format!("failed to read {}", characterdata.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let candidate = read_sidebar_layout(&path.join("sidebars.json"))?;
        best = best_sidebar_layout([best, candidate]);
    }

    Ok(best)
}

fn read_sidebar_layout(path: &Path) -> Result<Option<SidebarLayout>> {
    if !path.is_file() {
        return Ok(None);
    }

    let body = fs::read_to_string(path)
        .with_context(|| format!("failed to read sidebar layout {}", path.display()))?;
    let value: Value = serde_json::from_str(&body)
        .with_context(|| format!("failed to parse sidebar layout {}", path.display()))?;
    let score = sidebar_layout_score(&value, body.len() as u64);
    if score == 0 {
        return Ok(None);
    }

    Ok(Some(SidebarLayout {
        value,
        score,
        modified: fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok(),
    }))
}

fn best_sidebar_layout<I>(layouts: I) -> Option<SidebarLayout>
where
    I: IntoIterator<Item = Option<SidebarLayout>>,
{
    layouts
        .into_iter()
        .flatten()
        .max_by(|left, right| match left.score.cmp(&right.score) {
            std::cmp::Ordering::Equal => left.modified.cmp(&right.modified),
            ordering => ordering,
        })
}

fn sidebar_layout_score(value: &Value, byte_len: u64) -> u64 {
    let container_widgets = count_container_widgets(value) as u64;
    if container_widgets == 0 {
        return 0;
    }

    let container_options = value
        .get("containersOptions")
        .and_then(Value::as_object)
        .map(Map::len)
        .unwrap_or(0) as u64;
    let total_widgets = count_sidebar_widgets(value) as u64;

    container_widgets.saturating_mul(1_000_000)
        + container_options.saturating_mul(10_000)
        + total_widgets.saturating_mul(100)
        + byte_len.min(99)
}

fn count_container_widgets(value: &Value) -> usize {
    sidebar_widgets(value)
        .filter(|widget| widget.get("type").and_then(Value::as_str) == Some("container"))
        .count()
}

fn count_sidebar_widgets(value: &Value) -> usize {
    sidebar_widgets(value).count()
}

fn sidebar_widgets(value: &Value) -> impl Iterator<Item = &Value> {
    value
        .get("sidebarWidgetsMangerOptions")
        .and_then(|manager| manager.get("openWidgetsOrderPerSidebar"))
        .and_then(Value::as_array)
        .into_iter()
        .flat_map(|sidebars| sidebars.iter())
        .filter_map(Value::as_array)
        .flat_map(|widgets| widgets.iter())
}

fn apply_sidebar_layout_template(
    state_path: &Path,
    game_path: &Path,
    template: &Value,
) -> Result<()> {
    let characterdata = game_path.join("characterdata");
    fs::create_dir_all(&characterdata)
        .with_context(|| format!("failed to create {}", characterdata.display()))?;

    let mut ids = collect_numeric_character_ids(&characterdata)?;
    let Some(current_max) = ids.iter().copied().max() else {
        return Ok(());
    };

    let vault_path = latest_vault_path(state_path);
    let stored_preseed_until = load_sidebar_preseed_until(&vault_path)?;
    let preseed_until = if current_max > stored_preseed_until {
        current_max.saturating_add(FUTURE_CHARACTERDATA_SLOTS)
    } else {
        stored_preseed_until
    };

    for id in current_max.saturating_add(1)..=preseed_until {
        ids.insert(id);
    }

    for id in ids {
        let path = characterdata.join(id.to_string()).join("sidebars.json");
        apply_sidebar_layout_to_file(&path, template)?;
    }

    save_sidebar_preseed_until(&vault_path, preseed_until)?;
    Ok(())
}

fn collect_numeric_character_ids(characterdata: &Path) -> Result<HashSet<u32>> {
    let mut ids = HashSet::new();
    for entry in fs::read_dir(characterdata)
        .with_context(|| format!("failed to read {}", characterdata.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if let Ok(id) = name.parse::<u32>() {
            ids.insert(id);
        }
    }

    Ok(ids)
}

fn apply_sidebar_layout_to_file(path: &Path, template: &Value) -> Result<bool> {
    let current = if path.is_file() {
        let body = fs::read_to_string(path)
            .with_context(|| format!("failed to read sidebar layout {}", path.display()))?;
        serde_json::from_str(&body)
            .with_context(|| format!("failed to parse sidebar layout {}", path.display()))?
    } else {
        Value::Object(Map::new())
    };
    let merged = merge_sidebar_layout(current.clone(), template);

    if merged == current {
        return Ok(false);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    write_json_pretty(path, &merged)?;
    Ok(true)
}

fn merge_sidebar_layout(mut current: Value, template: &Value) -> Value {
    if !current.is_object() {
        current = Value::Object(Map::new());
    }

    let Some(template_object) = template.as_object() else {
        return current;
    };

    if current.as_object().is_some_and(Map::is_empty) {
        return template.clone();
    }

    if let Some(current_object) = current.as_object_mut() {
        for (key, template_value) in template_object {
            if is_sidebar_layout_key(key) {
                current_object.insert(key.clone(), template_value.clone());
            }
        }
    }

    current
}

fn is_sidebar_layout_key(key: &str) -> bool {
    key.ends_with("Options")
}

fn load_sidebar_preseed_until(vault_path: &Path) -> Result<u32> {
    let path = sidebar_template_meta_path(vault_path);
    if !path.is_file() {
        return Ok(0);
    }

    let body = fs::read_to_string(&path).with_context(|| {
        format!(
            "failed to read sidebar template metadata {}",
            path.display()
        )
    })?;
    let value: Value = serde_json::from_str(&body).with_context(|| {
        format!(
            "failed to parse sidebar template metadata {}",
            path.display()
        )
    })?;

    Ok(value
        .get("preseedUntil")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0))
}

fn save_sidebar_preseed_until(vault_path: &Path, preseed_until: u32) -> Result<()> {
    let path = sidebar_template_meta_path(vault_path);
    let value = serde_json::json!({ "preseedUntil": preseed_until });
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    write_json_pretty(&path, &value)
}

fn write_json_pretty(path: &Path, value: &Value) -> Result<()> {
    let body = serde_json::to_string_pretty(value)
        .with_context(|| format!("failed to serialize {}", path.display()))?;
    fs::write(path, body).with_context(|| format!("failed to write {}", path.display()))
}

fn should_restore_vault(
    vault_stats: &ClientUiStateStats,
    active_stats: &ClientUiStateStats,
) -> bool {
    if vault_stats.files == 0 {
        return false;
    }
    if active_stats.files == 0 {
        return true;
    }
    if active_stats.files + 2 < vault_stats.files {
        return true;
    }

    active_stats.bytes.saturating_mul(4) < vault_stats.bytes.saturating_mul(3)
}

fn should_run_discovery(
    vault_stats: &ClientUiStateStats,
    active_stats: &ClientUiStateStats,
) -> bool {
    vault_stats.files == 0 || active_stats.files == 0 || active_stats.files + 2 < vault_stats.files
}

fn summarize_state(root: &Path) -> Result<ClientUiStateStats> {
    let files = collect_state_files(root)?;
    Ok(ClientUiStateStats {
        files: files.len(),
        bytes: files.iter().map(|file| file.length).sum(),
    })
}

fn collect_state_files(root: &Path) -> Result<Vec<StateFile>> {
    let mut files = Vec::new();

    let client_options = root.join("conf").join("clientoptions.json");
    if client_options.is_file() {
        push_state_file(root, &client_options, &mut files)?;
    }

    let characterdata = root.join("characterdata");
    if characterdata.is_dir() {
        collect_json_files(root, &characterdata, 0, &mut files)?;
    }

    Ok(files)
}

fn collect_json_files(
    root: &Path,
    directory: &Path,
    depth: usize,
    files: &mut Vec<StateFile>,
) -> Result<()> {
    if depth > MAX_CHARACTERDATA_DEPTH {
        return Ok(());
    }

    for entry in fs::read_dir(directory)
        .with_context(|| format!("failed to read {}", directory.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            collect_json_files(root, &path, depth + 1, files)?;
        } else if metadata.is_file()
            && path
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| value.eq_ignore_ascii_case("json"))
                .unwrap_or(false)
        {
            push_state_file(root, &path, files)?;
        }
    }

    Ok(())
}

fn push_state_file(root: &Path, path: &Path, files: &mut Vec<StateFile>) -> Result<()> {
    let metadata = fs::metadata(path)?;
    let relative_path = path
        .strip_prefix(root)
        .with_context(|| format!("{} is not under {}", path.display(), root.display()))?
        .to_path_buf();

    files.push(StateFile {
        relative_path,
        length: metadata.len(),
        modified: metadata.modified().ok(),
    });
    Ok(())
}

fn copy_state_files(
    source_root: &Path,
    destination_root: &Path,
    mode: CopyMode,
) -> Result<ClientUiStateStats> {
    let source_files = collect_state_files(source_root)?;
    let mut copied = ClientUiStateStats::default();

    for source_file in source_files {
        let source = source_root.join(&source_file.relative_path);
        let destination = destination_root.join(&source_file.relative_path);

        if !should_copy_file(&source_file, &source, &destination, mode)? {
            continue;
        }

        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        if same_path(&source, &destination) {
            continue;
        }

        fs::copy(&source, &destination).with_context(|| {
            format!(
                "failed to copy client UI state {} to {}",
                source.display(),
                destination.display()
            )
        })?;
        copied.files += 1;
        copied.bytes += source_file.length;
    }

    Ok(copied)
}

fn should_copy_file(
    source_file: &StateFile,
    source: &Path,
    destination: &Path,
    mode: CopyMode,
) -> Result<bool> {
    if mode == CopyMode::RestoreOverwriteAll {
        return Ok(true);
    }

    let Ok(destination_metadata) = fs::metadata(destination) else {
        return Ok(true);
    };

    if mode == CopyMode::RestoreMissingOnly {
        return Ok(false);
    }

    let destination_modified = destination_metadata.modified().ok();
    let source_is_newer = match (source_file.modified, destination_modified) {
        (Some(source), Some(destination)) => source > destination,
        (Some(_), None) => true,
        _ => false,
    };

    if source_is_newer || source_file.length > destination_metadata.len() {
        return Ok(true);
    }

    files_differ(source, destination)
}

fn files_differ(left: &Path, right: &Path) -> Result<bool> {
    let left_bytes =
        fs::read(left).with_context(|| format!("failed to read {}", left.display()))?;
    let right_bytes =
        fs::read(right).with_context(|| format!("failed to read {}", right.display()))?;
    Ok(left_bytes != right_bytes)
}

fn discover_best_state_source(state_path: &Path, game_path: &Path) -> Result<Option<PathBuf>> {
    let active_stats = summarize_state(game_path)?;
    let roots = default_discovery_roots(state_path, game_path);
    discover_best_state_source_from_roots(&roots, game_path, &active_stats)
}

fn discover_best_state_source_from_roots(
    roots: &[PathBuf],
    game_path: &Path,
    active_stats: &ClientUiStateStats,
) -> Result<Option<PathBuf>> {
    let mut visited = HashSet::new();
    let mut best: Option<(PathBuf, ClientUiStateStats, u64)> = None;

    for root in roots {
        for candidate in candidate_paths(root, MAX_DISCOVERY_DEPTH)? {
            let normalized = normalized_path_key(&candidate);
            if !visited.insert(normalized) {
                continue;
            }
            if same_path(&candidate, game_path) || !is_penultima_client_root(&candidate) {
                continue;
            }

            let stats = summarize_state(&candidate)?;
            if !is_better_source(&stats, active_stats) {
                continue;
            }

            let score = state_score(&stats);
            if best
                .as_ref()
                .map(|(_, _, best_score)| score > *best_score)
                .unwrap_or(true)
            {
                best = Some((candidate, stats, score));
            }
        }
    }

    Ok(best.map(|(path, _, _)| path))
}

fn default_discovery_roots(state_path: &Path, game_path: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(base) = state_path.parent() {
        roots.push(base.to_path_buf());
    }
    roots.push(game_path.to_path_buf());

    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            roots.push(parent.to_path_buf());
            if let Some(grandparent) = parent.parent() {
                roots.push(grandparent.to_path_buf());
            }
        }
    }

    if let Some(home) = dirs::home_dir() {
        roots.push(home.join("Desktop"));
        roots.push(home.join("Downloads"));
        roots.push(home.join("Documents"));
    }

    for path in [r"C:\Penultima", r"C:\Games", r"C:\Tibia"] {
        roots.push(PathBuf::from(path));
    }

    roots
}

fn candidate_paths(root: &Path, max_depth: usize) -> Result<Vec<PathBuf>> {
    let mut candidates = Vec::new();
    collect_candidate_paths(root, 0, max_depth, &mut candidates)?;
    Ok(candidates)
}

fn collect_candidate_paths(
    path: &Path,
    depth: usize,
    max_depth: usize,
    candidates: &mut Vec<PathBuf>,
) -> Result<()> {
    if !path.exists() || depth > max_depth {
        return Ok(());
    }

    let normalized = normalize_client_root_path(path.to_path_buf());
    candidates.push(normalized);

    if depth == max_depth || !path.is_dir() {
        return Ok(());
    }

    let Ok(entries) = fs::read_dir(path) else {
        return Ok(());
    };

    for entry in entries.flatten() {
        let child = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() && !is_skipped_discovery_dir(&child) {
            collect_candidate_paths(&child, depth + 1, max_depth, candidates)?;
        }
    }

    Ok(())
}

fn is_skipped_discovery_dir(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();

    matches!(
        name.to_ascii_lowercase().as_str(),
        ".git" | "assets" | "bin" | "cache" | "crashdump" | "log" | "screenshots" | "sounds"
    )
}

fn is_penultima_client_root(path: &Path) -> bool {
    if !path.join("bin").join("client.exe").is_file() {
        return false;
    }

    let config_path = path.join("conf").join("config.ini");
    let Ok(config) = fs::read_to_string(config_path) else {
        return false;
    };
    let config = config.to_ascii_lowercase();
    config.contains("ultimaotserv.online") || config.contains("penultima")
}

fn is_better_source(candidate: &ClientUiStateStats, active: &ClientUiStateStats) -> bool {
    if candidate.files == 0 {
        return false;
    }
    if active.files == 0 {
        return true;
    }
    let meaningful_byte_gain = active.bytes.saturating_div(10).max(256);
    candidate.files > active.files + 2
        || (candidate.files >= active.files
            && candidate.bytes > active.bytes.saturating_add(meaningful_byte_gain))
}

fn state_score(stats: &ClientUiStateStats) -> u64 {
    (stats.files as u64).saturating_mul(1_000_000) + stats.bytes
}

fn normalize_client_root_path(path: PathBuf) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if name.eq_ignore_ascii_case("bin") {
        return path.parent().unwrap_or(&path).to_path_buf();
    }
    path
}

fn normalized_path_key(path: &Path) -> String {
    let normalized = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    normalized
        .to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn same_path(left: &Path, right: &Path) -> bool {
    normalized_path_key(left) == normalized_path_key(right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("penultima-ui-state-{name}-{unique}"))
    }

    fn write_file(root: &Path, relative: &str, body: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn write_penultima_marker(root: &Path) {
        write_file(root, "bin/client.exe", "exe");
        write_file(
            root,
            "conf/config.ini",
            "loginWebService=https://ultimaotserv.online/login.php",
        );
    }

    fn read_json(root: &Path, relative: &str) -> Value {
        serde_json::from_str(&fs::read_to_string(root.join(relative)).unwrap()).unwrap()
    }

    #[test]
    fn snapshots_and_restores_layout_state_files() {
        let root = temp_root("snapshot-restore");
        let game = root.join("game");
        let state = root.join("state");
        write_file(&game, "conf/clientoptions.json", r#"{"hotkeyOptions":{}}"#);
        write_file(
            &game,
            "characterdata/268435505/actionbars.json",
            r#"{"firstVisibleButtons":{"1":1}}"#,
        );
        write_file(
            &game,
            "characterdata/268435505/statusBarData.json",
            r#"{"position":"top"}"#,
        );
        write_file(
            &game,
            "characterdata/268435505/sidebars.json",
            r#"{"sidebarWidgetsMangerOptions":{}}"#,
        );

        let snapshot = snapshot_client_ui_state(&state, &game).unwrap();
        assert_eq!(snapshot.files, 4);

        fs::remove_dir_all(game.join("characterdata")).unwrap();
        fs::remove_file(game.join("conf/clientoptions.json")).unwrap();

        let restored = restore_client_ui_state(&state, &game).unwrap();
        assert_eq!(restored.files, 4);
        assert!(game.join("conf/clientoptions.json").is_file());
        assert!(
            game.join("characterdata/268435505/actionbars.json")
                .is_file()
        );
        assert!(
            game.join("characterdata/268435505/statusBarData.json")
                .is_file()
        );
        assert!(game.join("characterdata/268435505/sidebars.json").is_file());
    }

    #[test]
    fn vault_replaces_poor_current_state() {
        let root = temp_root("poor-current");
        let game = root.join("game");
        let state = root.join("state");
        let vault = latest_vault_path(&state);

        write_file(
            &vault,
            "conf/clientoptions.json",
            r#"{"hotkeyOptions":{"x":1}}"#,
        );
        write_file(
            &vault,
            "characterdata/268435505/actionbars.json",
            r#"{"firstVisibleButtons":{"1":1}}"#,
        );
        write_file(
            &vault,
            "characterdata/268435505/statusBarData.json",
            r#"{"position":"top"}"#,
        );
        write_file(
            &vault,
            "characterdata/268435505/sidebars.json",
            r#"{"sidebarWidgetsMangerOptions":{"openWidgetsOrderPerSidebar":[[]]}}"#,
        );
        write_file(&game, "characterdata/268435505/actionbars.json", r#"{}"#);

        let restored = restore_client_ui_state(&state, &game).unwrap();
        assert_eq!(restored.files, 4);
        let actionbars =
            fs::read_to_string(game.join("characterdata/268435505/actionbars.json")).unwrap();
        assert!(actionbars.contains("firstVisibleButtons"));
    }

    #[test]
    fn discovery_finds_old_penultima_client_without_manual_import() {
        let root = temp_root("discover");
        let current = root.join("current-game");
        let old = root.join("Downloads").join("Penultima Client");
        write_penultima_marker(&old);
        write_file(
            &old,
            "conf/clientoptions.json",
            r#"{"hotkeyOptions":{"currentHotkeySetName":"Knight"}}"#,
        );
        write_file(
            &old,
            "characterdata/268435505/sidebars.json",
            r#"{"sidebarWidgetsMangerOptions":{"openWidgetsOrderPerSidebar":[[{"type":"container"}]]}}"#,
        );
        write_file(
            &old,
            "characterdata/268435505/statusBarData.json",
            r#"{"position":"top"}"#,
        );

        let active = ClientUiStateStats::default();
        let found =
            discover_best_state_source_from_roots(&[root.join("Downloads")], &current, &active)
                .unwrap()
                .unwrap();

        assert_eq!(normalized_path_key(&found), normalized_path_key(&old));
    }

    #[test]
    fn discovery_can_replace_default_current_state_when_vault_is_empty() {
        let root = temp_root("discover-richer");
        let current = root.join("current-game");
        let old = root.join("Downloads").join("Penultima Client");
        write_penultima_marker(&old);
        write_file(
            &current,
            "conf/clientoptions.json",
            r#"{"hotkeyOptions":{}}"#,
        );
        write_file(
            &current,
            "characterdata/268435505/actionbars.json",
            r#"{"firstVisibleButtons":{}}"#,
        );
        write_file(
            &old,
            "conf/clientoptions.json",
            r#"{"hotkeyOptions":{"hotkeySets":{"Knight":{"actionBarOptions":{"mappings":[{"actionBar":1,"actionButton":1,"actionsetting":{"chatText":"exura ico","sendAutomatically":true}},{"actionBar":1,"actionButton":2,"actionsetting":{"useObject":266,"useType":"UseOnYourself"}}]}}}}}"#,
        );
        write_file(
            &old,
            "characterdata/268435505/actionbars.json",
            r#"{"firstVisibleButtons":{"1":1,"2":1,"3":1}}"#,
        );
        write_file(
            &old,
            "characterdata/268435505/sidebars.json",
            r#"{"sidebarWidgetsMangerOptions":{"openWidgetsOrderPerSidebar":[[{"type":"battleList"}],[{"type":"container","instance":0},{"type":"xpAnalyser"},{"type":"damageInputAnalyser"}]]},"containersOptions":{"0":{"contentHeight":200,"contentMaximized":true}}}"#,
        );

        let active = summarize_state(&current).unwrap();
        let found =
            discover_best_state_source_from_roots(&[root.join("Downloads")], &current, &active)
                .unwrap()
                .unwrap();

        assert_eq!(normalized_path_key(&found), normalized_path_key(&old));
    }

    #[test]
    fn sidebar_template_restores_container_position_to_new_runtime_folder() {
        let root = temp_root("sidebar-template-restore");
        let game = root.join("game");
        let state = root.join("state");

        let rich_sidebar = r#"{
            "containersOptions": {
                "2": { "contentHeight": 56, "contentMaximized": true },
                "3": { "contentHeight": 330, "contentMaximized": true },
                "4": { "contentHeight": 84, "contentMaximized": true }
            },
            "sidebarWidgetsMangerOptions": {
                "leftSidebarCount": 0,
                "openWidgetsOrderPerSidebar": [
                    [
                        { "instance": 0, "type": "battleList" },
                        { "instance": 3, "type": "container" }
                    ],
                    [
                        { "type": "playerGuide" },
                        { "type": "questTracker" },
                        { "instance": 2, "type": "container" },
                        { "instance": 4, "type": "container" }
                    ]
                ]
            },
            "skillsWidgetOptions": { "contentHeight": 198, "contentMaximized": true }
        }"#;

        write_file(&game, "conf/clientoptions.json", r#"{"hotkeyOptions":{}}"#);
        write_file(&game, "characterdata/100/sidebars.json", rich_sidebar);
        write_file(
            &game,
            "characterdata/101/sidebars.json",
            r#"{
                "containersOptions": { "0": { "contentHeight": 47, "contentMaximized": true } },
                "sidebarWidgetsMangerOptions": {
                    "leftSidebarCount": 0,
                    "openWidgetsOrderPerSidebar": [[{ "instance": 0, "type": "battleList" }]]
                },
                "skillsWidgetOptions": { "contentHeight": 10, "contentMaximized": false }
            }"#,
        );

        snapshot_client_ui_state(&state, &game).unwrap();
        restore_client_ui_state(&state, &game).unwrap();

        let rich = serde_json::from_str::<Value>(rich_sidebar).unwrap();
        let restored = read_json(&game, "characterdata/101/sidebars.json");
        assert_eq!(
            restored["sidebarWidgetsMangerOptions"],
            rich["sidebarWidgetsMangerOptions"]
        );
        assert_eq!(restored["containersOptions"], rich["containersOptions"]);
        assert_eq!(restored["skillsWidgetOptions"], rich["skillsWidgetOptions"]);

        let future = read_json(&game, "characterdata/357/sidebars.json");
        assert_eq!(
            future["sidebarWidgetsMangerOptions"],
            rich["sidebarWidgetsMangerOptions"]
        );
        assert_eq!(future["containersOptions"], rich["containersOptions"]);
        assert_eq!(future["skillsWidgetOptions"], rich["skillsWidgetOptions"]);
    }

    #[test]
    fn snapshot_merge_copies_same_size_sidebar_content_changes() {
        let root = temp_root("same-size-copy");
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.json");
        let destination = root.join("destination.json");
        fs::write(&source, br#"{"contentHeight":250}"#).unwrap();
        fs::write(&destination, br#"{"contentHeight":300}"#).unwrap();

        let source_file = StateFile {
            relative_path: PathBuf::from("sidebars.json"),
            length: fs::metadata(&source).unwrap().len(),
            modified: None,
        };

        assert!(
            should_copy_file(&source_file, &source, &destination, CopyMode::SnapshotMerge).unwrap()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sidebar_template_preseed_ceiling_does_not_grow_every_run() {
        let root = temp_root("sidebar-preseed-ceiling");
        let game = root.join("game");
        let state = root.join("state");
        let sidebar = r#"{
            "containersOptions": { "0": { "contentHeight": 200, "contentMaximized": true } },
            "sidebarWidgetsMangerOptions": {
                "leftSidebarCount": 0,
                "openWidgetsOrderPerSidebar": [[{ "instance": 0, "type": "container" }]]
            }
        }"#;

        write_file(&game, "conf/clientoptions.json", r#"{"hotkeyOptions":{}}"#);
        write_file(&game, "characterdata/200/sidebars.json", sidebar);

        sync_sidebar_layout_state(&state, &game).unwrap();
        assert!(game.join("characterdata/456/sidebars.json").is_file());
        assert!(!game.join("characterdata/457/sidebars.json").exists());

        sync_sidebar_layout_state(&state, &game).unwrap();
        assert!(game.join("characterdata/456/sidebars.json").is_file());
        assert!(!game.join("characterdata/712/sidebars.json").exists());

        write_file(&game, "characterdata/500/sidebars.json", sidebar);
        sync_sidebar_layout_state(&state, &game).unwrap();
        assert!(game.join("characterdata/756/sidebars.json").is_file());
    }
}
