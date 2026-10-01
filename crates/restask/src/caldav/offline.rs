//! A [`CaldavPort`] for machines with no CalDAV endpoint configured: every call fails
//! with a network error, so commands still do their vault-side work and report that the
//! server part is pending.

use chrono::{DateTime, Utc};

use crate::caldav::port::{CaldavPort, CollectionInfo, RemoteResource};
use crate::domain::{ListSlug, Task};
use crate::{CaldavErrorKind, RestaskError};

/// The "no server" port.
#[derive(Debug, Clone, Copy, Default)]
pub struct Offline;

fn unavailable<T>() -> Result<T, RestaskError> {
    Err(RestaskError::Caldav {
        kind: CaldavErrorKind::Network,
        status: None,
        detail: "no CalDAV endpoint configured on this machine (run `restask setup`)".to_string(),
    })
}

impl CaldavPort for Offline {
    async fn list_collections(&self) -> Result<Vec<CollectionInfo>, RestaskError> {
        unavailable()
    }

    async fn ensure_collection(
        &self,
        _slug: &ListSlug,
        _display: &str,
    ) -> Result<(), RestaskError> {
        unavailable()
    }

    async fn list_tasks(
        &self,
        _slug: &ListSlug,
    ) -> Result<Option<Vec<RemoteResource>>, RestaskError> {
        unavailable()
    }

    async fn put(
        &self,
        _task: &Task,
        _name: &str,
        _extras: &[String],
        _if_match: Option<&str>,
        _now: DateTime<Utc>,
    ) -> Result<String, RestaskError> {
        unavailable()
    }

    async fn delete(
        &self,
        _slug: &ListSlug,
        _name: &str,
        _etag: Option<&str>,
    ) -> Result<(), RestaskError> {
        unavailable()
    }
}
