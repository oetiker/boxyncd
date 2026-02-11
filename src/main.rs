use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand};
use sqlx::Row;

mod auth;
mod box_api;
mod config;
mod db;
mod service;
mod sync;
mod util;

#[derive(Parser)]
#[command(
    name = "boxyncd",
    version,
    about = "Bidirectional Box.com file sync daemon for Linux"
)]
struct Cli {
    /// Path to config file [default: ~/.config/boxyncd/config.toml]
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Increase log verbosity (-v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Authenticate with Box.com (opens browser for OAuth)
    Auth,
    /// Start the sync daemon (foreground, for systemd)
    Start,
    /// Show sync status summary
    Status,
    /// Trigger an immediate full sync cycle
    SyncNow,
    /// Manage the systemd user service
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
}

#[derive(Subcommand)]
enum ServiceAction {
    /// Install and enable the systemd user service
    Install,
    /// Stop and remove the systemd user service
    Uninstall,
    /// Show service status
    Status,
    /// Tail service logs
    Log,
}

fn init_tracing(verbosity: u8) {
    let default_filter = match verbosity {
        0 => "boxyncd=info",
        1 => "boxyncd=debug",
        2 => "boxyncd=trace",
        _ => "trace",
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default_filter.into()),
        )
        .init();
}

/// Check inotify watch limits on Linux and warn if they look too low.
fn check_inotify_limits() {
    let path = "/proc/sys/fs/inotify/max_user_watches";
    if let Ok(content) = std::fs::read_to_string(path)
        && let Ok(limit) = content.trim().parse::<u64>()
    {
        if limit < 65536 {
            tracing::warn!(
                max_user_watches = limit,
                "inotify watch limit is low — you may hit issues with large trees. \
                 Increase with: echo 524288 | sudo tee {path}"
            );
        } else {
            tracing::debug!(max_user_watches = limit, "inotify watch limit OK");
        }
    }
}

/// Clean up stale pending_operations from a previous crash.
async fn cleanup_stale_operations(pool: &sqlx::SqlitePool) -> Result<()> {
    let result = sqlx::query(
        "UPDATE pending_operations SET state = 'failed', \
         updated_at = datetime('now') \
         WHERE state = 'in_progress'",
    )
    .execute(pool)
    .await?;

    let affected = result.rows_affected();
    if affected > 0 {
        tracing::warn!(
            count = affected,
            "marked stale in-progress operations as failed (from prior crash)"
        );
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    // Service commands don't need config
    if let Command::Service { action } = cli.command {
        return match action {
            ServiceAction::Install => service::install(),
            ServiceAction::Uninstall => service::uninstall(),
            ServiceAction::Status => service::status(),
            ServiceAction::Log => service::log(),
        };
    }

    // Auth only needs auth config, not sync roots — works without a config file
    // when built-in credentials are available (official release builds).
    if let Command::Auth = cli.command {
        let cfg = match config::load_config(cli.config.as_deref()) {
            Ok(c) => c,
            Err(_) if config::has_builtin_credentials() => config::auth_only_config(),
            Err(e) => return Err(e),
        };
        return auth::run_auth_flow(&cfg).await;
    }

    let cfg = config::load_config(cli.config.as_deref())?;

    match cli.command {
        Command::Auth => unreachable!("handled above"),
        Command::Start => {
            check_inotify_limits();

            let pool = db::init_db().await?;
            cleanup_stale_operations(&pool).await?;

            let token_mgr = Arc::new(auth::TokenManager::new(&cfg)?);
            let client = Arc::new(box_api::BoxClient::new(token_mgr));
            let engine = Arc::new(sync::SyncEngine::new(
                pool.clone(),
                client.clone(),
                cfg.clone(),
            ));

            tracing::info!("boxyncd daemon ready — running initial sync");
            if let Err(e) = engine.run_full_sync().await {
                tracing::error!(error = %e, "initial sync failed");
            }

            // Start local file watchers
            let roots: Vec<(usize, PathBuf)> = cfg
                .sync
                .iter()
                .enumerate()
                .map(|(i, r)| (i, r.local_path.clone()))
                .collect();
            let (mut local_rx, _watcher_handle) =
                sync::local_watcher::start_local_watchers(&roots, cfg.general.local_debounce_ms)?;

            // Start remote event watchers
            let cancel = tokio_util::sync::CancellationToken::new();
            let mut remote_rx = sync::remote_watcher::start_remote_watchers(
                client,
                pool.clone(),
                cfg.sync.len(),
                cancel.clone(),
            );

            // SIGTERM handling (for systemd graceful stop)
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

            // Main event loop
            let interval = cfg.general.full_sync_interval_secs;
            tracing::info!(interval_secs = interval, "entering sync loop");

            let mut pending_roots = std::collections::HashSet::new();
            let mut sync_timer = tokio::time::interval(std::time::Duration::from_secs(interval));
            sync_timer.tick().await; // consume the initial instant tick

            loop {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {
                        tracing::info!("received SIGINT, shutting down");
                        cancel.cancel();
                        break;
                    }

                    _ = sigterm.recv() => {
                        tracing::info!("received SIGTERM, shutting down");
                        cancel.cancel();
                        break;
                    }

                    // Local change detected
                    Some(change) = local_rx.recv() => {
                        tracing::debug!(
                            root_index = change.root_index,
                            path = %change.path.display(),
                            "local change detected"
                        );
                        pending_roots.insert(change.root_index);
                    }

                    // Remote change detected
                    Some(change) = remote_rx.recv() => {
                        tracing::debug!(
                            root_index = change.root_index,
                            "remote change signal received"
                        );
                        pending_roots.insert(change.root_index);
                    }

                    // Periodic full sync timer
                    _ = sync_timer.tick() => {
                        tracing::debug!("periodic full sync");
                        if let Err(e) = engine.run_full_sync().await {
                            tracing::error!(error = %e, "periodic sync failed");
                        }
                        pending_roots.clear();
                    }

                    // Short delay to batch incremental changes
                    _ = tokio::time::sleep(std::time::Duration::from_millis(500)),
                        if !pending_roots.is_empty() => {
                        let roots_to_sync: Vec<usize> = pending_roots.drain().collect();
                        for root_index in roots_to_sync {
                            tracing::info!(root_index, "incremental sync triggered");
                            if let Err(e) = engine.sync_one_root(root_index as i64).await {
                                tracing::error!(
                                    root_index,
                                    error = %e,
                                    "incremental sync failed"
                                );
                            }
                        }
                    }
                }
            }

            tracing::info!("closing database");
            pool.close().await;
            tracing::info!("boxyncd stopped");
        }
        Command::Status => {
            let pool = db::init_db().await?;
            print_status(&pool, &cfg).await?;
            pool.close().await;
        }
        Command::SyncNow => {
            let pool = db::init_db().await?;
            cleanup_stale_operations(&pool).await?;

            let token_mgr = Arc::new(auth::TokenManager::new(&cfg)?);
            let client = Arc::new(box_api::BoxClient::new(token_mgr));
            let engine = sync::SyncEngine::new(pool.clone(), client, cfg.clone());

            tracing::info!("running full sync");
            engine.run_full_sync().await?;

            pool.close().await;
            println!("sync complete");
        }
        Command::Service { .. } => unreachable!("handled above"),
    }

    Ok(())
}

/// Print a detailed sync status summary.
async fn print_status(pool: &sqlx::SqlitePool, cfg: &config::Config) -> Result<()> {
    // Overall counts
    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sync_entries")
        .fetch_one(pool)
        .await?;
    let files: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM sync_entries WHERE entry_type = 'file'")
            .fetch_one(pool)
            .await?;
    let folders: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM sync_entries WHERE entry_type = 'folder'")
            .fetch_one(pool)
            .await?;
    let errors: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM sync_entries WHERE sync_status = 'error'")
            .fetch_one(pool)
            .await?;
    let conflicts: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM sync_entries WHERE sync_status = 'conflict'")
            .fetch_one(pool)
            .await?;

    println!("boxyncd status");
    println!("=============");
    println!(
        "Tracked: {} total ({} files, {} folders)",
        total.0, files.0, folders.0
    );

    if errors.0 > 0 {
        println!("Errors:  {}", errors.0);
    }
    if conflicts.0 > 0 {
        println!("Conflicts: {}", conflicts.0);
    }

    // Per-root breakdown
    for (i, root) in cfg.sync.iter().enumerate() {
        let root_index = i as i64;
        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM sync_entries WHERE sync_root_index = ?")
                .bind(root_index)
                .fetch_one(pool)
                .await?;

        let root_errors: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sync_entries WHERE sync_root_index = ? AND sync_status = 'error'",
        )
        .bind(root_index)
        .fetch_one(pool)
        .await?;

        let last_sync = sqlx::query(
            "SELECT MAX(last_sync_at) as last FROM sync_entries WHERE sync_root_index = ?",
        )
        .bind(root_index)
        .fetch_one(pool)
        .await?;
        let last_sync_at: Option<String> = last_sync.get("last");

        println!();
        println!(
            "Root #{}: {} -> box:{}",
            i,
            root.local_path.display(),
            root.box_folder_id
        );
        println!("  Entries: {}", count.0);
        if root_errors.0 > 0 {
            println!("  Errors:  {}", root_errors.0);
        }
        if let Some(ts) = last_sync_at {
            println!("  Last sync: {ts}");
        } else {
            println!("  Last sync: never");
        }

        // Show stream position if available
        if let Ok(Some(pos)) = sync::state::get_stream_position(pool, root_index).await {
            println!("  Stream position: {pos}");
        }
    }

    // Show errored entries
    if errors.0 > 0 {
        println!();
        println!("Recent errors:");
        let err_rows = sqlx::query(
            "SELECT relative_path, last_error, retry_count FROM sync_entries \
             WHERE sync_status = 'error' ORDER BY last_sync_at DESC LIMIT 10",
        )
        .fetch_all(pool)
        .await?;

        for row in &err_rows {
            let path: &str = row.get("relative_path");
            let error: Option<&str> = row.get("last_error");
            let retries: i64 = row.get("retry_count");
            println!(
                "  {} (retries: {}): {}",
                path,
                retries,
                error.unwrap_or("unknown")
            );
        }
    }

    Ok(())
}
