use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use notify_debouncer_mini::{DebouncedEventKind, new_debouncer};
use tokio::sync::mpsc;

/// A change event from the local filesystem watcher.
#[derive(Debug, Clone)]
pub struct LocalChange {
    /// Which sync root this change belongs to (index into config.sync).
    pub root_index: usize,
    /// Absolute path of the changed file/directory.
    pub path: PathBuf,
}

/// Start watching all sync roots for local filesystem changes.
///
/// Returns a receiver that produces `LocalChange` events when files are
/// created, modified, or deleted. Events are debounced to coalesce
/// rapid-fire inotify bursts (e.g., editor save = truncate + write).
///
/// The watcher runs until the returned `WatcherHandle` is dropped.
pub fn start_local_watchers(
    roots: &[(usize, PathBuf)],
    debounce_ms: u64,
) -> Result<(mpsc::UnboundedReceiver<LocalChange>, WatcherHandle)> {
    let (tx, rx) = mpsc::unbounded_channel();

    // Build a mapping from watched root paths to root indices,
    // so we can tag events with the correct root.
    let root_map: Vec<(usize, PathBuf)> = roots.to_vec();

    let tx_clone = tx.clone();
    let root_map_clone = root_map.clone();

    let mut debouncer = new_debouncer(
        Duration::from_millis(debounce_ms),
        move |result: Result<Vec<notify_debouncer_mini::DebouncedEvent>, notify::Error>| {
            match result {
                Ok(events) => {
                    for event in events {
                        if event.kind != DebouncedEventKind::Any
                            && event.kind != DebouncedEventKind::AnyContinuous
                        {
                            continue;
                        }

                        // Determine which sync root this path belongs to
                        if let Some(root_index) = find_root_for_path(&event.path, &root_map_clone) {
                            let _ = tx_clone.send(LocalChange {
                                root_index,
                                path: event.path,
                            });
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "local watcher error");
                }
            }
        },
    )
    .context("Failed to create file watcher")?;

    // Watch each root recursively
    let watcher = debouncer.watcher();
    for (index, root_path) in &root_map {
        watcher
            .watch(root_path.as_ref(), notify::RecursiveMode::Recursive)
            .with_context(|| format!("Failed to watch root {}: {}", index, root_path.display()))?;
        tracing::info!(root = %root_path.display(), "watching for local changes");
    }

    Ok((
        rx,
        WatcherHandle {
            _debouncer: debouncer,
        },
    ))
}

/// Handle that keeps the watcher alive. Drop to stop watching.
pub struct WatcherHandle {
    _debouncer: notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>,
}

/// Find which sync root a given path belongs to.
fn find_root_for_path(path: &Path, roots: &[(usize, PathBuf)]) -> Option<usize> {
    roots
        .iter()
        .find(|(_, root_path)| path.starts_with(root_path))
        .map(|(index, _)| *index)
}
