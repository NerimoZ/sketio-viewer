use std::{
    ffi::OsString,
    fs, io,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct Workspace {
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub zoom: u32,
}

#[derive(Deserialize, Serialize)]
pub struct Tab {
    path: Option<Vec<u16>>,
    pub position: [i32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub untitled: Option<bool>,
}

impl Tab {
    pub fn new(path: Option<&Path>, position: [i32; 2], untitled: Option<bool>) -> Self {
        Self {
            path: path.map(|path| path.as_os_str().encode_wide().collect()),
            position,
            untitled,
        }
    }

    pub fn path(&self) -> Option<PathBuf> {
        self.path
            .as_deref()
            .map(|wide| PathBuf::from(OsString::from_wide(wide)))
    }
}

pub fn load(directory: Option<&Path>) -> Option<Workspace> {
    let file = directory?.join("session.json");
    let content = fs::read(file).ok()?;
    serde_json::from_slice(&content).ok()
}

pub fn save(directory: Option<&Path>, workspace: &Workspace) -> io::Result<()> {
    let Some(directory) = directory else {
        return Ok(());
    };
    save_file(directory, "session.json", serde_json::to_vec(workspace)?)
}

pub fn save_file(directory: &Path, name: &str, data: impl AsRef<[u8]>) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    let temp = directory.join(format!("{name}-{}.tmp", std::process::id()));
    let result = fs::write(&temp, data).and_then(|()| fs::rename(&temp, directory.join(name)));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
