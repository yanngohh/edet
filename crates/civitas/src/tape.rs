//! **The tape: a run as one directory.**
//!
//! ```text
//! <run>/manifest.json   what the run is: its whole configuration, its model,
//!                       the hashes of what every tier was told
//! <run>/events.log      typed events, in the order they happened
//! <run>/exchange.log    the raw exchange of every day and every card, by id
//! <run>/player/         derived by `index`, and regenerable from the three above
//! ```
//!
//! Both logs frame every record as `[len: u32 LE][sha256(payload): 32][payload]`,
//! the node store's framing, with a JSON payload. A tail torn by a crash in the
//! middle of an append is ordinary and is dropped when the log is opened; a
//! record whose hash does not match anywhere before the tail is corruption and
//! refuses the run.
//!
//! **An exchange is written before the event that names it**, so a day that
//! reached the tape is always readable; an exchange no event names is a day
//! that did not finish, and nothing reads it.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::config::RunConfig;

pub const TAPE_FORMAT: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub tape_format: u32,
    pub config: RunConfig,
    /// The backend and model the run was started under. A resume under a
    /// different one writes a `ModelChanged` event, or is refused.
    pub backend: String,
    pub model: String,
    /// The sha256 of what each tier is told, so a later reader can tell two
    /// runs apart that were told different things.
    pub knowledge: Vec<(String, String)>,
    /// `true` for a scripted backend: this tape is not a run.
    pub not_a_run: bool,
}

pub struct Tape {
    pub dir: PathBuf,
    events: File,
    exchange: File,
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).into()
}

/// Read every whole record of a framed log, and the offset its whole records
/// end at. A torn or mismatched LAST record is dropped; a mismatch anywhere
/// else is an error.
fn read_frames(path: &Path) -> Result<(Vec<Vec<u8>>, u64), String> {
    let mut bytes = Vec::new();
    match File::open(path) {
        Ok(mut f) => {
            f.read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    }
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        if bytes.len() - at < 36 {
            break;
        }
        let len = u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes")) as usize;
        let start = at + 36;
        if bytes.len() - start < len {
            break;
        }
        let payload = &bytes[start..start + len];
        if sha256(payload) != bytes[at + 4..at + 36] {
            if start + len == bytes.len() {
                break;
            }
            return Err(format!("{}: a record at offset {at} does not match its hash", path.display()));
        }
        out.push(payload.to_vec());
        at = start + len;
    }
    Ok((out, at as u64))
}

fn open_append(path: &Path, whole: u64) -> Result<File, String> {
    let mut f = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.set_len(whole).map_err(|e| format!("{}: {e}", path.display()))?;
    f.seek(SeekFrom::End(0)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(f)
}

fn append(f: &mut File, value: &impl Serialize) -> Result<(), String> {
    let payload = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    let mut frame = Vec::with_capacity(36 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&sha256(&payload));
    frame.extend_from_slice(&payload);
    f.write_all(&frame).map_err(|e| e.to_string())?;
    f.sync_data().map_err(|e| e.to_string())
}

impl Tape {
    /// Start a run in a directory that holds no run yet.
    pub fn create(dir: &Path, manifest: &Manifest) -> Result<Tape, String> {
        if dir.join("manifest.json").exists() {
            return Err(format!("{} already holds a run; `resume` continues it", dir.display()));
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let text = serde_json::to_string_pretty(manifest).map_err(|e| e.to_string())?;
        let tmp = dir.join("manifest.json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, dir.join("manifest.json")).map_err(|e| e.to_string())?;
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
        Tape::open(dir)
    }

    /// Open a run for appending, dropping any torn tail.
    pub fn open(dir: &Path) -> Result<Tape, String> {
        let (_, events_end) = read_frames(&dir.join("events.log"))?;
        let (_, exchange_end) = read_frames(&dir.join("exchange.log"))?;
        Ok(Tape {
            dir: dir.to_path_buf(),
            events: open_append(&dir.join("events.log"), events_end)?,
            exchange: open_append(&dir.join("exchange.log"), exchange_end)?,
        })
    }

    pub fn event(&mut self, e: &crate::event::Event) -> Result<(), String> {
        append(&mut self.events, e)
    }

    pub fn exchange(&mut self, x: &Exchange) -> Result<(), String> {
        append(&mut self.exchange, x)
    }
}

pub fn read_manifest(dir: &Path) -> Result<Manifest, String> {
    let path = dir.join("manifest.json");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let m: Manifest = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if m.tape_format != TAPE_FORMAT {
        return Err(format!("{}: tape format {} is not {TAPE_FORMAT}", path.display(), m.tape_format));
    }
    Ok(m)
}

fn read_all<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>, String> {
    let (frames, _) = read_frames(path)?;
    frames
        .iter()
        .enumerate()
        .map(|(i, f)| serde_json::from_slice(f).map_err(|e| format!("{} record {i}: {e}", path.display())))
        .collect()
}

pub fn read_events(dir: &Path) -> Result<Vec<crate::event::Event>, String> {
    read_all(&dir.join("events.log"))
}

pub fn read_exchanges(dir: &Path) -> Result<Vec<Exchange>, String> {
    read_all(&dir.join("exchange.log"))
}

/// The raw exchange of one model conversation step: a person's day, or the
/// writing of a newcomer's card. `messages` are exactly what that step added
/// to the conversation, in the API's own shape.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Exchange {
    pub id: u64,
    pub person: usize,
    pub tick: u64,
    pub what: ExchangeKind,
    pub messages: Vec<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExchangeKind {
    Day,
    Card,
}
