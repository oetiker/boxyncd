use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use sqlx::SqlitePool;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::box_api::BoxClient;

use super::state;

/// A signal from the remote watcher indicating new changes on Box.
#[derive(Debug, Clone)]
pub struct RemoteChange {
    /// Which sync root detected the change (index into config.sync).
    pub root_index: usize,
}

/// Start the remote long-poll watcher for all sync roots.
///
/// Spawns one task per sync root that continuously long-polls Box for
/// change events. When changes are detected, sends a `RemoteChange`
/// on the returned channel, which triggers a full sync for that root.
///
/// The watcher runs until the cancellation token is cancelled.
pub fn start_remote_watchers(
    client: Arc<BoxClient>,
    pool: SqlitePool,
    root_count: usize,
    cancel: CancellationToken,
) -> mpsc::UnboundedReceiver<RemoteChange> {
    let (tx, rx) = mpsc::unbounded_channel();

    for root_index in 0..root_count {
        let client = client.clone();
        let pool = pool.clone();
        let tx = tx.clone();
        let cancel = cancel.clone();

        tokio::spawn(async move {
            if let Err(e) = long_poll_loop(client, pool, root_index, tx, cancel).await {
                tracing::error!(
                    root_index,
                    error = %e,
                    "remote watcher terminated with error"
                );
            }
        });
    }

    rx
}

/// The long-poll loop for a single sync root.
///
/// Flow:
/// 1. Get or initialize the stream position from DB
/// 2. Get the long-poll server URL (OPTIONS /events)
/// 3. Long-poll (blocks until changes or timeout)
/// 4. On change: fetch events, update stream position, notify channel
/// 5. Repeat (re-acquire long-poll URL periodically as it expires)
async fn long_poll_loop(
    client: Arc<BoxClient>,
    pool: SqlitePool,
    root_index: usize,
    tx: mpsc::UnboundedSender<RemoteChange>,
    cancel: CancellationToken,
) -> Result<()> {
    let root_index_i64 = root_index as i64;

    // Initialize stream position if we don't have one
    let mut stream_pos = match state::get_stream_position(&pool, root_index_i64).await? {
        Some(pos) => {
            tracing::debug!(root_index, position = %pos, "resuming from saved stream position");
            pos
        }
        None => {
            let pos = client.get_stream_position().await?;
            state::set_stream_position(&pool, root_index_i64, &pos).await?;
            tracing::info!(root_index, position = %pos, "initialized stream position");
            pos
        }
    };

    loop {
        if cancel.is_cancelled() {
            tracing::debug!(root_index, "remote watcher cancelled");
            return Ok(());
        }

        // Step 1: Get long-poll server configuration
        let server = match client.get_long_poll_info().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    root_index,
                    error = %e,
                    "failed to get long-poll info, retrying in 30s"
                );
                tokio::select! {
                    _ = cancel.cancelled() => return Ok(()),
                    _ = tokio::time::sleep(Duration::from_secs(30)) => continue,
                }
            }
        };

        // Step 2: Determine how many retries we can do on this server URL
        let max_retries: u32 = server
            .max_retries
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);

        for _retry in 0..max_retries {
            if cancel.is_cancelled() {
                return Ok(());
            }

            // Step 3: Long-poll (blocks for up to ~10 minutes)
            tracing::debug!(root_index, "long-polling for remote changes");
            let has_changes = tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                result = client.long_poll(&server) => {
                    match result {
                        Ok(changed) => changed,
                        Err(e) => {
                            tracing::warn!(
                                root_index,
                                error = %e,
                                "long-poll failed, will re-acquire server URL"
                            );
                            break; // break inner loop to re-acquire server URL
                        }
                    }
                }
            };

            if !has_changes {
                // Timeout / reconnect — no new changes, poll again
                continue;
            }

            // Step 4: Fetch the actual events
            tracing::debug!(root_index, "new remote changes detected, fetching events");
            match client.get_sync_events(&stream_pos).await {
                Ok((events, next_pos)) => {
                    if !events.is_empty() {
                        tracing::info!(
                            root_index,
                            count = events.len(),
                            "received remote change events"
                        );
                        for event in &events {
                            tracing::debug!(
                                root_index,
                                event_type = %event.event_type,
                                source = ?event.source.as_ref().map(|s| &s.name),
                                "remote event"
                            );
                        }
                    }

                    // Update stream position
                    stream_pos = next_pos;
                    if let Err(e) =
                        state::set_stream_position(&pool, root_index_i64, &stream_pos).await
                    {
                        tracing::error!(
                            root_index,
                            error = %e,
                            "failed to save stream position"
                        );
                    }

                    // Notify the sync engine to run a sync cycle
                    if !events.is_empty() {
                        let _ = tx.send(RemoteChange { root_index });
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        root_index,
                        error = %e,
                        "failed to fetch events, will retry"
                    );
                }
            }
        }

        // After max_retries, loop back to re-acquire the long-poll server URL
        tracing::debug!(root_index, "re-acquiring long-poll server URL");
    }
}
