use crate::{
    clients::{self, Server, Source},
    storage::{self, Change, Pending, State, Store},
};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::{fs, path::PathBuf};

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ClientView {
    pub source: Source,
    pub servers: Vec<Server>,
    pub error: Option<String>,
    #[serde(skip)]
    pub contents: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub clients: Vec<ClientView>,
}

pub struct Manager {
    sources: Vec<Source>,
    pub store: Store,
}

impl Manager {
    pub fn open(sources: Vec<Source>, directory: PathBuf) -> Result<Self> {
        let manager = Self {
            sources,
            store: Store::open(directory)?,
        };
        manager.recover()?;
        Ok(manager)
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let state = self.store.load()?;
        if state.pending.is_some() {
            bail!("A previous change needs recovery; restart MCP Manager");
        }
        let mut clients = Vec::new();
        for configured in &self.sources {
            let mut source = configured.clone();
            let result = (|| {
                source.path =
                    fs::canonicalize(&configured.path).context("No configuration found")?;
                let contents = storage::read_config(&source.path)?;
                let servers = clients::inventory(&source, &contents, &state.parked)?;
                Ok::<_, anyhow::Error>((contents, servers))
            })();
            match result {
                Ok((contents, servers)) => clients.push(ClientView {
                    source,
                    servers,
                    error: None,
                    contents: Some(contents),
                }),
                Err(error) => clients.push(ClientView {
                    source,
                    servers: Vec::new(),
                    error: Some(error.to_string()),
                    contents: None,
                }),
            }
        }
        Ok(Snapshot { clients })
    }

    pub fn toggle(&self, view: &ClientView, server: &Server) -> Result<()> {
        self.recover()?;
        let configured = self
            .sources
            .iter()
            .find(|s| s.kind == view.source.kind)
            .context("Unknown client")?;
        if fs::canonicalize(&configured.path)? != view.source.path {
            bail!("Configuration location changed. Wait for the menu to update and try again.");
        }
        let expected = view
            .contents
            .as_ref()
            .context("Configuration was not loaded")?;
        if storage::read_config(&view.source.path)? != *expected {
            bail!(
                "Configuration changed outside MCP Manager. Wait for the menu to update and try again."
            );
        }
        let state = self.store.load()?;
        let current = clients::inventory(&view.source, expected, &state.parked)?;
        if !current.contains(server) {
            bail!("Server changed. Wait for the menu to update and try again.");
        }
        let mut parked = state.parked.clone();
        let after = clients::edit(&view.source, expected, server, &mut parked)?;
        // Validate the entire resulting client inventory before writing anything.
        clients::inventory(&view.source, &after, &parked)?;
        self.commit(Change {
            path: view.source.path.clone(),
            before: expected.clone(),
            after,
            parked_before: state.parked,
            parked_after: parked,
        })
    }

    fn commit(&self, change: Change) -> Result<()> {
        if storage::read_config(&change.path)? != change.before {
            bail!(
                "Configuration changed outside MCP Manager. Wait for the menu to update and try again."
            );
        }
        let mut state = self.store.load()?;
        if state.pending.is_some() {
            bail!("A previous change must be recovered first");
        }
        if state.parked != change.parked_before {
            bail!("Disabled definitions changed; wait for the menu to update before toggling");
        }
        state.pending = Some(Pending {
            change: change.clone(),
        });
        self.store.save(&state)?;
        // A durable journal is saved before replacing the client file. If either write fails,
        // recover() compares exact bytes and finalizes or cancels without guessing.
        if let Err(error) = storage::replace_config(&change.path, &change.before, &change.after) {
            // The next automatic update can still inspect the unchanged state if this was a conflict.
            // Only clear a pending write if the replacement did not reach disk.
            if storage::read_config(&change.path).is_ok_and(|text| text == change.before) {
                state.pending = None;
                self.store.save(&state)?;
            }
            return Err(error);
        }
        self.recover()
    }

    pub fn recover(&self) -> Result<()> {
        let mut state: State = self.store.load()?;
        let Some(pending) = state.pending.take() else {
            return Ok(());
        };
        let current = storage::read_config(&pending.change.path)
            .context("Cannot recover previous change. Keep the application data folder and restore the configuration path.")?;
        if current == pending.change.after {
            state.parked = pending.change.parked_after.clone();
        } else if current != pending.change.before {
            bail!(
                "A configuration changed during an interrupted save. Recovery is paused to protect your edits. Preserve state.json; see docs/recovery.md."
            );
        }
        self.store.save(&state)
    }
}
