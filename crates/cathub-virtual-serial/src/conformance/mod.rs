//! Serial API conformance profiles and result reports.

mod profiles;

#[cfg(windows)]
mod windows;

use std::net::SocketAddr;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use profiles::{find_profile, profiles};

/// Stable identifier for a serial API test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseId {
    /// Blocking `ReadFile` and `WriteFile` transfer.
    SynchronousIo,
    /// Overlapped `ReadFile` and `WriteFile` transfer.
    OverlappedIo,
    /// Cancellation of a pending overlapped read.
    CancelPendingRead,
    /// Read timeout behavior.
    ReadTimeout,
    /// Receive queue purge and abort behavior.
    PurgeReceive,
    /// Receive notification through `WaitCommEvent`.
    WaitCommEvent,
    /// Serial line format and flow-control configuration.
    SerialConfiguration,
    /// DTR, RTS, and break control requests.
    ModemControl,
    /// Queue counters and error status through `ClearCommError`.
    QueueStatus,
    /// Bounded-buffer overflow fails atomically and leaves the handle usable.
    BufferSaturation,
}

impl CaseId {
    /// Return the stable command-line and report identifier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SynchronousIo => "synchronous_io",
            Self::OverlappedIo => "overlapped_io",
            Self::CancelPendingRead => "cancel_pending_read",
            Self::ReadTimeout => "read_timeout",
            Self::PurgeReceive => "purge_receive",
            Self::WaitCommEvent => "wait_comm_event",
            Self::SerialConfiguration => "serial_configuration",
            Self::ModemControl => "modem_control",
            Self::QueueStatus => "queue_status",
            Self::BufferSaturation => "buffer_saturation",
        }
    }
}

/// Importance of one case for an application profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Requirement {
    /// The client cannot be supported when this case fails.
    Required,
    /// The client can operate without this case, but CatHub records the result.
    Optional,
    /// The client does not use this behavior in the current inventory.
    NotApplicable,
}

/// One serial behavior in an application profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProfileCase {
    /// Test identifier.
    pub id: CaseId,
    /// Importance for the client.
    pub requirement: Requirement,
    /// Current source for the requirement.
    pub evidence: &'static str,
}

/// Expected serial behavior for one supported application interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApplicationProfile {
    /// Stable profile name.
    pub name: &'static str,
    /// Application and interface name.
    pub client: &'static str,
    /// Required serial format.
    pub line_format: &'static str,
    /// Tests selected for this profile.
    pub cases: &'static [ProfileCase],
}

/// Result state for one conformance case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseStatus {
    /// The device met the test condition.
    Passed,
    /// The device did not meet the test condition.
    Failed,
    /// The harness could not run the test in this environment.
    Skipped,
}

/// Result for one serial API test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseResult {
    /// Test identifier.
    pub id: CaseId,
    /// Profile requirement.
    pub requirement: Requirement,
    /// Result state.
    pub status: CaseStatus,
    /// Human-readable result detail.
    pub detail: String,
    /// Win32 error code when an API call failed.
    pub win32_error: Option<u32>,
}

/// Machine-readable output from one harness run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConformanceReport {
    /// Report schema version.
    pub schema_version: u32,
    /// UTC time from the host that ran the harness.
    pub generated_at_utc: String,
    /// Selected application profile.
    pub profile: String,
    /// Application-facing COM port.
    pub application_port: String,
    /// Peer COM port used by the test process.
    pub peer_port: String,
    /// Operating system description.
    pub operating_system: String,
    /// Result for each selected case.
    pub results: Vec<CaseResult>,
}

impl ConformanceReport {
    /// Return true when all required tests pass.
    #[must_use]
    pub fn required_cases_pass(&self) -> bool {
        self.results.iter().all(|result| {
            result.requirement != Requirement::Required || result.status == CaseStatus::Passed
        })
    }

    /// Save a formatted JSON report.
    ///
    /// # Errors
    ///
    /// Returns an error when the destination cannot be created or written.
    pub fn write_json(&self, path: &Path) -> Result<(), ConformanceError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, format!("{text}\n"))?;
        Ok(())
    }
}

/// Run one profile against an isolated Windows virtual serial pair.
///
/// # Errors
///
/// Returns an error if the profile is unknown, a port cannot open, or the host is not Windows.
pub fn run(
    profile_name: &str,
    application_port: &str,
    peer_port: &str,
) -> Result<ConformanceReport, ConformanceError> {
    let profile = find_profile(profile_name)
        .ok_or_else(|| ConformanceError::UnknownProfile(profile_name.to_string()))?;

    #[cfg(windows)]
    {
        windows::run_serial_pair(profile, application_port, peer_port)
    }

    #[cfg(not(windows))]
    {
        let _ = (profile, application_port, peer_port);
        Err(ConformanceError::UnsupportedPlatform)
    }
}

/// Run one profile against a CatHub-managed endpoint whose private peer is exposed over TCP.
///
/// The TCP bridge is a test-only stand-in for `cathub.exe`'s private UMDF adapter. The
/// application-facing side remains the real Windows COM device and exercises the same native
/// serial APIs as the two-port harness.
///
/// # Errors
///
/// Returns an error if the profile is unknown, the COM port or peer cannot open, or the host is
/// not Windows.
pub fn run_with_tcp_peer(
    profile_name: &str,
    application_port: &str,
    peer_address: SocketAddr,
) -> Result<ConformanceReport, ConformanceError> {
    let profile = find_profile(profile_name)
        .ok_or_else(|| ConformanceError::UnknownProfile(profile_name.to_string()))?;

    #[cfg(windows)]
    {
        Ok(windows::run_tcp_peer(
            profile,
            application_port,
            peer_address,
        ))
    }

    #[cfg(not(windows))]
    {
        let _ = (profile, application_port, peer_address);
        Err(ConformanceError::UnsupportedPlatform)
    }
}

/// Conformance harness errors.
#[derive(Debug, Error)]
pub enum ConformanceError {
    /// The requested profile does not exist.
    #[error("unknown conformance profile '{0}'")]
    UnknownProfile(String),
    /// The native serial API harness only runs on Windows.
    #[error("the serial conformance harness requires Windows")]
    UnsupportedPlatform,
    /// A Win32 operation failed.
    #[error("Win32 operation '{operation}' failed with error {code}")]
    Win32 {
        /// Operation name.
        operation: &'static str,
        /// `GetLastError` value.
        code: u32,
    },
    /// A native operation returned an invalid result.
    #[error("serial conformance failure: {0}")]
    InvalidResult(String),
    /// Report file operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Report serialization failed.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn every_profile_has_unique_cases() {
        for profile in profiles() {
            for (index, case) in profile.cases.iter().enumerate() {
                assert!(
                    profile.cases[..index].iter().all(|seen| seen.id != case.id),
                    "profile {} has duplicate case {}",
                    profile.name,
                    case.id.as_str()
                );
            }
        }
    }

    #[test]
    fn required_case_summary_ignores_optional_failure() {
        let report = ConformanceReport {
            schema_version: 1,
            generated_at_utc: String::new(),
            profile: "test".to_string(),
            application_port: "COM1".to_string(),
            peer_port: "COM2".to_string(),
            operating_system: "test".to_string(),
            results: vec![
                CaseResult {
                    id: CaseId::SynchronousIo,
                    requirement: Requirement::Required,
                    status: CaseStatus::Passed,
                    detail: String::new(),
                    win32_error: None,
                },
                CaseResult {
                    id: CaseId::ModemControl,
                    requirement: Requirement::Optional,
                    status: CaseStatus::Failed,
                    detail: String::new(),
                    win32_error: Some(1),
                },
            ],
        };

        assert!(report.required_cases_pass());
    }
}
