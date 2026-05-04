use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::SystemTime;

use notify::{Config as NotifyConfig, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::watch;

use crate::{LoadedWorkflow, WorkflowError, load_effective_workflow_with_env};

#[derive(Debug)]
pub struct WorkflowReloader {
    workflow_path: PathBuf,
    env: HashMap<String, String>,
    last_good: LoadedWorkflow,
    last_modified: Option<SystemTime>,
    change_rx: watch::Receiver<()>,
    _watcher: Option<RecommendedWatcher>,
}

impl WorkflowReloader {
    pub fn start(explicit: Option<PathBuf>) -> Result<Self, WorkflowError> {
        Self::start_with_env(explicit, std::env::vars().collect())
    }

    pub fn start_with_env(
        explicit: Option<PathBuf>,
        env: HashMap<String, String>,
    ) -> Result<Self, WorkflowError> {
        let last_good = load_effective_workflow_with_env(explicit, &env)?;
        let workflow_path = last_good.definition.path.clone();
        let last_modified = last_good.modified_at;
        let (change_tx, change_rx) = watch::channel(());
        let (notify_tx, notify_rx) = mpsc::channel();
        let mut watcher = RecommendedWatcher::new(notify_tx, NotifyConfig::default()).ok();
        if let Some(watcher) = watcher.as_mut() {
            let _ = watcher.watch(&workflow_path, RecursiveMode::NonRecursive);
        }
        std::thread::spawn(move || {
            while notify_rx.recv().is_ok() {
                let _ = change_tx.send(());
            }
        });
        Ok(Self {
            workflow_path,
            env,
            last_good,
            last_modified,
            change_rx,
            _watcher: watcher,
        })
    }

    pub fn current(&self) -> &LoadedWorkflow {
        &self.last_good
    }

    pub fn reload_if_changed(&mut self) -> Result<bool, WorkflowError> {
        let changed_by_watch = self.change_rx.has_changed().unwrap_or(false);
        if changed_by_watch {
            let _ = self.change_rx.borrow_and_update();
        }
        let modified = fs::metadata(&self.workflow_path)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        let changed_by_mtime = modified.is_some() && modified != self.last_modified;
        if !changed_by_watch && !changed_by_mtime {
            return Ok(false);
        }
        match load_effective_workflow_with_env(Some(self.workflow_path.clone()), &self.env) {
            Ok(loaded) => {
                self.last_modified = loaded.modified_at;
                self.last_good = loaded;
                Ok(true)
            }
            Err(err) => {
                tracing::error!(error=%err, "workflow_reload outcome=failed");
                Err(err)
            }
        }
    }
}
