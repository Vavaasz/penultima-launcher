use anyhow::{Context, Result};
use log::info;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const VAULT_DIR: &str = "client-ui-state";
const LATEST_DIR: &str = "latest";
const MAX_DISCOVERY_DEPTH: usize = 2;
const MAX_CHARACTERDATA_DEPTH: usize = 4;

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
    copy_state_files(game_path, &vault_path, CopyMode::SnapshotMerge)
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

    copy_state_files(&vault_path, game_path, mode)
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

        if !should_copy_file(&source_file, &destination, mode)? {
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

fn should_copy_file(source_file: &StateFile, destination: &Path, mode: CopyMode) -> Result<bool> {
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

    Ok(source_is_newer || source_file.length > destination_metadata.len())
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
}
