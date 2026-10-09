//! What a device mints UIDs under (§9.4): its tag, and the claim that makes the tag its
//! own in the vault.
//!
//! The device keeps its identity — the tag, a secret of its own, and the last number it
//! used — in a file beside its machine config, which no file sync carries. The claim is
//! `.restask/devices/<tag>` in the vault, holding the secret: it rides the file sync, so
//! every device sees which tags are taken. Two devices that took the same tag before
//! they saw each other write that file twice; the file sync keeps one, and the device
//! whose secret is not in it takes another tag at its next look.

use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::domain::uid::{Counter, DeviceTag};
use crate::fsio::write_atomic;
use crate::RestaskError;

/// Directory of the claims under `.restask/`.
const DEVICES_DIR: &str = "devices";

/// A device's identity in one vault (§9.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// The tag its UIDs carry.
    tag: DeviceTag,
    /// What its claim file holds: known to this device only.
    secret: String,
    /// The highest number it has used under the tag.
    last: u64,
}

impl Device {
    /// The identity this device works under in the vault whose state is `state_dir`,
    /// claimed on the spot when it has none — or lost the one it had.
    ///
    /// `file` is where the identity is kept between runs; without one it is `held`, the
    /// identity an earlier call returned (a process that keeps none on disk). `taken`
    /// are the tags UIDs of the vault already carry: a new tag is drawn at random among
    /// the shortest ones that neither a claim nor a UID uses.
    pub fn open(
        state_dir: &Path,
        file: Option<&Path>,
        held: Option<Device>,
        taken: &BTreeSet<String>,
    ) -> Result<Device, RestaskError> {
        let known = match file {
            Some(file) => read_identity(file)?,
            None => held,
        };
        let dir = state_dir.join(DEVICES_DIR);
        let secret = match known {
            Some(device) => match std::fs::read_to_string(dir.join(device.tag.as_str())) {
                Ok(claim) if claim.trim() == device.secret => return Ok(device),
                Ok(_) => {
                    tracing::warn!(tag = %device.tag, "device_tag_lost: another device holds it; taking a new one");
                    device.secret
                }
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    // The state directory was emptied, or this is the claim's first write.
                    std::fs::create_dir_all(&dir)?;
                    write_atomic(&dir.join(device.tag.as_str()), &claim_of(&device.secret))?;
                    return Ok(device);
                }
                Err(e) => return Err(e.into()),
            },
            None => ulid::Ulid::new().to_string().to_lowercase(),
        };

        let mut used = taken.clone();
        match std::fs::read_dir(&dir) {
            Ok(entries) => {
                for entry in entries {
                    // A conflict copy of a claim (`a.sync-conflict-…`) is a claim.
                    let name = entry?.file_name().to_string_lossy().to_string();
                    used.extend(name.split('.').next().map(str::to_string));
                }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let free = (1..=4)
            .map(|letters| {
                DeviceTag::all(letters)
                    .into_iter()
                    .filter(|tag| !used.contains(tag.as_str()))
                    .collect::<Vec<_>>()
            })
            .find(|free| !free.is_empty())
            .ok_or_else(|| RestaskError::Validation {
                field: "device",
                reason: "every device tag of this vault is taken".to_string(),
            })?;
        let draw = usize::try_from(ulid::Ulid::new().random() % (free.len() as u128)).unwrap_or(0);
        let device = Device {
            tag: free[draw].clone(),
            secret,
            last: 0,
        };
        // The identity first: a claim nobody remembers making would keep its tag for good.
        if let Some(file) = file {
            device.save(file)?;
        }
        std::fs::create_dir_all(&dir)?;
        write_atomic(&dir.join(device.tag.as_str()), &claim_of(&device.secret))?;
        tracing::info!(tag = %device.tag, "device_tag_claimed");
        Ok(device)
    }

    /// The tag this device's UIDs carry.
    pub fn tag(&self) -> &DeviceTag {
        &self.tag
    }

    /// A counter that goes on from the last number this device used.
    pub fn counter(&self) -> Counter {
        Counter::new(self.tag.clone(), self.last)
    }

    /// Remembers what `counter` handed out, in `file` when the identity is kept in one.
    /// Returns whether there was anything to remember.
    pub fn used(&mut self, counter: &Counter, file: Option<&Path>) -> Result<bool, RestaskError> {
        if counter.tag() != &self.tag || counter.last() <= self.last {
            return Ok(false);
        }
        self.last = counter.last();
        if let Some(file) = file {
            self.save(file)?;
        }
        Ok(true)
    }

    /// Writes the identity to `file`.
    fn save(&self, file: &Path) -> Result<(), RestaskError> {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_atomic(
            file,
            &format!("{} {} {}\n", self.tag, self.secret, self.last),
        )?;
        Ok(())
    }
}

/// Whether the vault whose state is `state_dir` has been switched to counted UIDs
/// (§9.4): some device holds a claim there. The first claim is the sync node's, made by
/// the pass that reached the server; a device that only edits mints counted UIDs once it
/// sees one — a sync node that does not read them yet has made none.
pub fn switched(state_dir: &Path) -> Result<bool, RestaskError> {
    let entries = match std::fs::read_dir(state_dir.join(DEVICES_DIR)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let name = entry?.file_name().to_string_lossy().to_string();
        if DeviceTag::parse(name.split('.').next().unwrap_or_default()).is_ok() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The file a device keeps its identity in (§9.4): `device`, beside the machine config
/// at `config` — per vault, like everything there (§14.2).
pub fn device_file(config: &Path) -> PathBuf {
    config.with_file_name("device")
}

/// The content of a claim file.
fn claim_of(secret: &str) -> String {
    format!("{secret}\n")
}

/// Reads an identity file: `<tag> <secret> <last>`. A missing file, or one that is not
/// that, is no identity.
fn read_identity(file: &Path) -> Result<Option<Device>, RestaskError> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut words = text.split_whitespace();
    let device = match (words.next(), words.next(), words.next()) {
        (Some(tag), Some(secret), Some(last)) => DeviceTag::parse(tag)
            .ok()
            .zip(last.parse().ok())
            .map(|(tag, last)| Device {
                tag,
                secret: secret.to_string(),
                last,
            }),
        _ => None,
    };
    if device.is_none() {
        tracing::warn!(path = %file.display(), "unreadable device identity; taking a new one");
    }
    Ok(device)
}
