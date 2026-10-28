use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub const MAX_CONFIG_SIZE: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParkedServer {
    pub source: PathBuf,
    pub name: String,
    // Keep the raw JSONC value, including comments inside the definition.
    pub definition: String,
}

pub type Parked = BTreeMap<String, ParkedServer>;

#[derive(Clone, Serialize, Deserialize)]
pub struct Change {
    pub path: PathBuf,
    pub before: String,
    pub after: String,
    pub parked_before: Parked,
    pub parked_after: Parked,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Pending {
    pub change: Change,
}

#[derive(Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub parked: Parked,
    pub pending: Option<Pending>,
}

pub struct Store {
    directory: PathBuf,
    _lock: File,
}

impl Store {
    pub fn open(directory: PathBuf) -> Result<Self> {
        private_directory(&directory)?;
        let lock_path = directory.join("instance.lock");
        reject_symlink(&lock_path)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(lock_path)
            .context("Cannot open application lock")?;
        lock.try_lock_exclusive()
            .context("MCP Manager is already running for this data folder")?;
        Ok(Self {
            directory,
            _lock: lock,
        })
    }

    pub fn load(&self) -> Result<State> {
        let path = self.directory.join("state.json");
        if !path.try_exists()? {
            return Ok(State {
                version: 1,
                ..State::default()
            });
        }
        reject_symlink(&path)?;
        let state: State = serde_json::from_str(&fs::read_to_string(path)?)
            .map_err(|_| anyhow::anyhow!("Application state is invalid. Preserve state.json; it may contain disabled server definitions."))?;
        if state.version != 1 {
            bail!(
                "Unsupported application state version; use the version that created this data folder"
            );
        }
        Ok(state)
    }

    pub fn save(&self, state: &State) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(state)?;
        atomic_write(&self.directory.join("state.json"), &bytes, 0o600)
    }
}

pub fn private_directory(path: &Path) -> Result<()> {
    reject_symlink(path)?;
    fs::create_dir_all(path).context("Cannot create private application directory")?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            bail!("Refusing to replace a symbolic link: {}", path.display())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub fn read_config(path: &Path) -> Result<String> {
    let meta = fs::metadata(path).with_context(|| format!("Cannot read {}", path.display()))?;
    if !meta.is_file() {
        bail!("Configuration is not a regular file");
    }
    if meta.len() > MAX_CONFIG_SIZE {
        bail!("Configuration exceeds the 8 MiB limit");
    }
    fs::read_to_string(path).context("Cannot read configuration as UTF-8")
}

pub fn replace_config(path: &Path, expected: &str, replacement: &str) -> Result<()> {
    reject_symlink(path)?;
    let meta = fs::metadata(path)?;
    if meta.nlink() != 1 {
        bail!("Hard-linked configurations need to be edited in the client");
    }
    if read_config(path)? != expected {
        bail!(
            "Configuration changed outside MCP Manager. Wait for the menu to update and try again."
        );
    }
    atomic_write(
        path,
        replacement.as_bytes(),
        meta.permissions().mode() & 0o777,
    )
}

pub fn atomic_write(path: &Path, contents: &[u8], mode: u32) -> Result<()> {
    reject_symlink(path)?;
    let parent = path.parent().context("File has no parent folder")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .context("Cannot create temporary file beside configuration")?;
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(mode))?;
    temporary.write_all(contents)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .context("Cannot save file atomically")?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
