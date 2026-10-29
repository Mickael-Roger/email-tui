//! Native directory notifications. Watches the collection, not individual files,
//! so an editor/synchronizer replacing an ICS/VCF file atomically is detected.
use anyhow::{Context, Result};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{collections::HashSet, path::PathBuf, time::Duration};
use tokio::{sync::mpsc, task::JoinHandle};

pub struct Changes {
    pub events: mpsc::Receiver<Result<(), String>>,
    task: JoinHandle<()>,
}
impl Changes {
    pub fn start(mut paths: Vec<PathBuf>) -> Result<Self> {
        paths.sort();
        paths.dedup();
        let targets = paths.clone();
        // A single pending notification represents any number of file changes.
        let (raw_tx, mut raw_rx) = mpsc::channel(1);
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                let result = match result {
                    Ok(event) => {
                        let changed = event.need_rescan()
                            || matches!(
                                event.kind,
                                EventKind::Create(_)
                                    | EventKind::Modify(_)
                                    | EventKind::Remove(_)
                                    | EventKind::Any
                            ) && event.paths.iter().any(|path| {
                                targets.iter().any(|target| {
                                    path.starts_with(target) || target.starts_with(path)
                                })
                            });
                        if !changed {
                            return;
                        }
                        Ok(())
                    }
                    Err(error) => Err(error.to_string()),
                };
                let _ = raw_tx.try_send(result);
            })
            .context("Start native PIM directory watcher")?;
        let mut watched = HashSet::new();
        rewatch(&mut watcher, &paths, &mut watched)?;
        let (events, receiver) = mpsc::channel(1);
        let task = tokio::spawn(async move {
            while let Some(mut result) = raw_rx.recv().await {
                // Coalesce write/rename bursts before reading a consistent snapshot.
                tokio::time::sleep(Duration::from_millis(200)).await;
                while let Ok(next) = raw_rx.try_recv() {
                    if next.is_err() {
                        result = next;
                    }
                }
                // Directory replacement can invalidate an inotify watch even if
                // another directory already exists at the same pathname.
                if let Err(error) = rewatch(&mut watcher, &paths, &mut watched) {
                    result = Err(format!("{error:#}"));
                }
                if events.send(result).await.is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            events: receiver,
            task,
        })
    }
}
impl Drop for Changes {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn rewatch(
    watcher: &mut RecommendedWatcher,
    targets: &[PathBuf],
    watched: &mut HashSet<PathBuf>,
) -> Result<()> {
    let mut desired = HashSet::new();
    for target in targets {
        if target.is_dir() {
            desired.insert(target.clone());
        }
        // Watch the parent as well, to detect collection deletion/recreation.
        // A missing collection is covered by its nearest existing ancestor.
        if let Some(parent) = target.ancestors().skip(1).find(|path| path.is_dir()) {
            desired.insert(parent.to_path_buf());
        }
    }
    for path in watched.drain() {
        let _ = watcher.unwatch(&path);
    }
    let mut failure = None;
    for path in desired {
        match watcher.watch(&path, RecursiveMode::NonRecursive) {
            Ok(()) => {
                watched.insert(path);
            }
            Err(error) => {
                failure =
                    Some(anyhow::Error::new(error).context(format!("Watch {}", path.display())));
            }
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(())
}
