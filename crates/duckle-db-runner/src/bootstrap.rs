//! Versioned one-shot bootstrap payloads for a sidecar child.
//!
//! The parent writes one bounded message to a child-only inherited pipe and
//! closes that pipe. Credentials are binary data, never environment values,
//! command-line arguments, readiness files, events, or serializable DTOs.

use crate::model::WorkerId;
use crate::resources::ResolvedRunnerResources;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

pub const BOOTSTRAP_PROTOCOL_VERSION: u32 = 1;
const CREDENTIAL_BYTES: usize = 32;
const MAX_BOOTSTRAP_BYTES: usize = 4 * 1024;
const MAX_READINESS_BYTES: usize = 2 * 1024;
type HmacSha256 = Hmac<sha2::Sha256>;

/// A 256-bit credential. It intentionally implements neither `Debug`,
/// `Display`, `Clone`, nor serialization.
pub struct BootstrapSecret([u8; CREDENTIAL_BYTES]);

impl BootstrapSecret {
    pub fn generate() -> Result<Self, BootstrapError> {
        let mut bytes = [0_u8; CREDENTIAL_BYTES];
        getrandom::fill(&mut bytes).map_err(|_| BootstrapError::RandomnessUnavailable)?;
        Ok(Self(bytes))
    }

    pub(crate) fn as_bytes(&self) -> &[u8; CREDENTIAL_BYTES] {
        &self.0
    }
}

impl Drop for BootstrapSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapMetadata {
    protocol_version: u32,
    worker_id: WorkerId,
    effective_profile: ResolvedRunnerResources,
}

/// The parent creates this once per worker. The credential stays private to
/// the provider and Quack connector; callers can inspect only safe metadata.
pub struct BootstrapMessage {
    metadata: BootstrapMetadata,
    credential: BootstrapSecret,
}

impl BootstrapMessage {
    pub fn new(
        worker_id: WorkerId,
        effective_profile: ResolvedRunnerResources,
    ) -> Result<Self, BootstrapError> {
        effective_profile
            .validate()
            .map_err(|_| BootstrapError::InvalidProfile)?;
        Ok(Self {
            metadata: BootstrapMetadata {
                protocol_version: BOOTSTRAP_PROTOCOL_VERSION,
                worker_id,
                effective_profile,
            },
            credential: BootstrapSecret::generate()?,
        })
    }

    pub fn worker_id(&self) -> WorkerId {
        self.metadata.worker_id
    }

    pub fn effective_profile(&self) -> &ResolvedRunnerResources {
        &self.metadata.effective_profile
    }

    #[allow(dead_code)] // consumed by the provider-private Quack connector
    pub(crate) fn credential(&self) -> &BootstrapSecret {
        &self.credential
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadinessMetadata {
    bootstrap_protocol_version: u32,
    runner_protocol_version: u32,
    worker_id: WorkerId,
    effective_profile: ResolvedRunnerResources,
    endpoint: SocketAddr,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadinessEnvelope {
    metadata: ReadinessMetadata,
    authentication_tag: Vec<u8>,
}

/// Provider-private proof that the child which received the one-shot
/// credential also bound a loopback endpoint and applied the complete profile.
/// It deliberately exposes no endpoint or authentication material.
pub struct AuthenticatedReadiness {
    metadata: ReadinessMetadata,
}

impl AuthenticatedReadiness {
    pub fn worker_id(&self) -> WorkerId {
        self.metadata.worker_id
    }

    pub fn effective_profile(&self) -> &ResolvedRunnerResources {
        &self.metadata.effective_profile
    }

    pub fn runner_protocol_version(&self) -> u32 {
        self.metadata.runner_protocol_version
    }

    pub(crate) fn endpoint(&self) -> SocketAddr {
        self.metadata.endpoint
    }
}

#[derive(Debug, Error)]
pub enum BootstrapError {
    #[error("runner bootstrap randomness is unavailable")]
    RandomnessUnavailable,
    #[error("runner bootstrap profile is invalid")]
    InvalidProfile,
    #[error("runner bootstrap transport failed")]
    Io(#[source] io::Error),
    #[error("runner bootstrap metadata is invalid")]
    InvalidMetadata,
    #[error("runner bootstrap protocol version is unsupported")]
    UnsupportedProtocol,
    #[error("runner bootstrap payload is malformed")]
    MalformedPayload,
    #[error("runner bootstrap payload has trailing bytes")]
    TrailingBytes,
    #[error("runner readiness endpoint is not a bound loopback address")]
    NonLoopbackEndpoint,
    #[error("runner readiness authentication failed")]
    AuthenticationFailed,
    #[error("runner protocol version is unsupported")]
    RunnerProtocolMismatch,
}

/// Writes one length-prefixed bootstrap record. The parent must close its
/// write endpoint immediately after this succeeds so the child can reject
/// appended data deterministically.
pub fn write_bootstrap<W: Write>(
    writer: &mut W,
    message: &BootstrapMessage,
) -> Result<(), BootstrapError> {
    let metadata =
        serde_json::to_vec(&message.metadata).map_err(|_| BootstrapError::InvalidMetadata)?;
    let payload_len = metadata
        .len()
        .checked_add(CREDENTIAL_BYTES)
        .and_then(|length| length.checked_add(2))
        .ok_or(BootstrapError::MalformedPayload)?;
    if payload_len > MAX_BOOTSTRAP_BYTES || metadata.len() > u16::MAX as usize {
        return Err(BootstrapError::MalformedPayload);
    }

    let mut payload = Zeroizing::new(Vec::with_capacity(payload_len));
    payload.extend_from_slice(&(metadata.len() as u16).to_be_bytes());
    payload.extend_from_slice(&metadata);
    payload.extend_from_slice(message.credential.as_bytes());
    writer
        .write_all(&(payload.len() as u32).to_be_bytes())
        .and_then(|_| writer.write_all(&payload))
        .map_err(BootstrapError::Io)
}

/// Reads exactly one bootstrap record. A writer that leaves data after the
/// record is rejected; this blocks until the parent closes its pipe endpoint.
pub fn read_bootstrap<R: Read>(reader: &mut R) -> Result<BootstrapMessage, BootstrapError> {
    let mut length = [0_u8; 4];
    reader.read_exact(&mut length).map_err(BootstrapError::Io)?;
    let payload_len = u32::from_be_bytes(length) as usize;
    if !(2 + CREDENTIAL_BYTES..=MAX_BOOTSTRAP_BYTES).contains(&payload_len) {
        return Err(BootstrapError::MalformedPayload);
    }

    let mut payload = Zeroizing::new(vec![0_u8; payload_len]);
    reader
        .read_exact(&mut payload)
        .map_err(BootstrapError::Io)?;
    let metadata_len = u16::from_be_bytes([payload[0], payload[1]]) as usize;
    if metadata_len + 2 + CREDENTIAL_BYTES != payload.len() {
        return Err(BootstrapError::MalformedPayload);
    }
    let metadata: BootstrapMetadata = serde_json::from_slice(&payload[2..2 + metadata_len])
        .map_err(|_| BootstrapError::InvalidMetadata)?;
    if metadata.protocol_version != BOOTSTRAP_PROTOCOL_VERSION {
        return Err(BootstrapError::UnsupportedProtocol);
    }
    metadata
        .effective_profile
        .validate()
        .map_err(|_| BootstrapError::InvalidProfile)?;
    let mut credential = [0_u8; CREDENTIAL_BYTES];
    credential.copy_from_slice(&payload[2 + metadata_len..]);

    let mut trailing = [0_u8; 1];
    match reader.read(&mut trailing).map_err(BootstrapError::Io)? {
        0 => Ok(BootstrapMessage {
            metadata,
            credential: BootstrapSecret(credential),
        }),
        _ => Err(BootstrapError::TrailingBytes),
    }
}

/// Writes the child-to-parent readiness response after resource application
/// and loopback bind have completed. The response is authenticated with the
/// one-shot bootstrap credential and may travel only on the inherited control
/// pipe.
pub fn write_authenticated_readiness<W: Write>(
    writer: &mut W,
    bootstrap: &BootstrapMessage,
    endpoint: SocketAddr,
) -> Result<(), BootstrapError> {
    validate_loopback_endpoint(endpoint)?;
    bootstrap
        .effective_profile()
        .validate()
        .map_err(|_| BootstrapError::InvalidProfile)?;
    let metadata = ReadinessMetadata {
        bootstrap_protocol_version: BOOTSTRAP_PROTOCOL_VERSION,
        runner_protocol_version: crate::RUNNER_PROTOCOL_VERSION,
        worker_id: bootstrap.worker_id(),
        effective_profile: bootstrap.effective_profile().clone(),
        endpoint,
    };
    let metadata_bytes =
        serde_json::to_vec(&metadata).map_err(|_| BootstrapError::InvalidMetadata)?;
    let mut mac = HmacSha256::new_from_slice(bootstrap.credential().as_bytes())
        .map_err(|_| BootstrapError::AuthenticationFailed)?;
    mac.update(&metadata_bytes);
    let envelope = ReadinessEnvelope {
        metadata,
        authentication_tag: mac.finalize().into_bytes().to_vec(),
    };
    let payload =
        Zeroizing::new(serde_json::to_vec(&envelope).map_err(|_| BootstrapError::InvalidMetadata)?);
    if payload.len() > MAX_READINESS_BYTES {
        return Err(BootstrapError::MalformedPayload);
    }
    writer
        .write_all(&(payload.len() as u32).to_be_bytes())
        .and_then(|_| writer.write_all(&payload))
        .map_err(BootstrapError::Io)
}

/// Reads and authenticates exactly one readiness response. The returned value
/// contains private connection metadata but has no serialization or debug
/// implementation, so it cannot cross IPC or logging boundaries accidentally.
pub fn read_authenticated_readiness<R: Read>(
    reader: &mut R,
    expected: &BootstrapMessage,
) -> Result<AuthenticatedReadiness, BootstrapError> {
    let mut length = [0_u8; 4];
    reader.read_exact(&mut length).map_err(BootstrapError::Io)?;
    let payload_len = u32::from_be_bytes(length) as usize;
    if payload_len == 0 || payload_len > MAX_READINESS_BYTES {
        return Err(BootstrapError::MalformedPayload);
    }
    let mut payload = Zeroizing::new(vec![0_u8; payload_len]);
    reader
        .read_exact(&mut payload)
        .map_err(BootstrapError::Io)?;
    let envelope: ReadinessEnvelope =
        serde_json::from_slice(&payload).map_err(|_| BootstrapError::InvalidMetadata)?;
    let metadata_bytes =
        serde_json::to_vec(&envelope.metadata).map_err(|_| BootstrapError::InvalidMetadata)?;
    let mut mac = HmacSha256::new_from_slice(expected.credential().as_bytes())
        .map_err(|_| BootstrapError::AuthenticationFailed)?;
    mac.update(&metadata_bytes);
    mac.verify_slice(&envelope.authentication_tag)
        .map_err(|_| BootstrapError::AuthenticationFailed)?;

    let metadata = envelope.metadata;
    if metadata.bootstrap_protocol_version != BOOTSTRAP_PROTOCOL_VERSION {
        return Err(BootstrapError::UnsupportedProtocol);
    }
    if metadata.runner_protocol_version != crate::RUNNER_PROTOCOL_VERSION {
        return Err(BootstrapError::RunnerProtocolMismatch);
    }
    if metadata.worker_id != expected.worker_id()
        || metadata.effective_profile != *expected.effective_profile()
    {
        return Err(BootstrapError::AuthenticationFailed);
    }
    validate_loopback_endpoint(metadata.endpoint)?;
    metadata
        .effective_profile
        .validate()
        .map_err(|_| BootstrapError::InvalidProfile)?;

    let mut trailing = [0_u8; 1];
    if reader.read(&mut trailing).map_err(BootstrapError::Io)? != 0 {
        return Err(BootstrapError::TrailingBytes);
    }
    Ok(AuthenticatedReadiness { metadata })
}

fn validate_loopback_endpoint(endpoint: SocketAddr) -> Result<(), BootstrapError> {
    if endpoint.ip().is_loopback() && endpoint.port() != 0 {
        Ok(())
    } else {
        Err(BootstrapError::NonLoopbackEndpoint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::{HostResourceLimits, RunnerResourcesProfile};
    use std::io::Cursor;

    fn default_effective_profile() -> ResolvedRunnerResources {
        RunnerResourcesProfile::default()
            .resolve(HostResourceLimits::default())
            .unwrap()
    }

    #[test]
    fn round_trips_only_a_complete_versioned_message() {
        let message = BootstrapMessage::new(WorkerId::new(), default_effective_profile()).unwrap();
        let worker_id = message.worker_id();
        let credential = *message.credential().as_bytes();
        let mut bytes = Vec::new();
        write_bootstrap(&mut bytes, &message).unwrap();

        let decoded = read_bootstrap(&mut Cursor::new(bytes)).unwrap();
        assert_eq!(decoded.worker_id(), worker_id);
        assert_eq!(decoded.effective_profile(), &default_effective_profile());
        assert_eq!(*decoded.credential().as_bytes(), credential);
    }

    #[test]
    fn rejects_truncated_oversized_and_appended_payloads() {
        let message = BootstrapMessage::new(WorkerId::new(), default_effective_profile()).unwrap();
        let mut encoded = Vec::new();
        write_bootstrap(&mut encoded, &message).unwrap();

        assert!(matches!(
            read_bootstrap(&mut Cursor::new(encoded[..encoded.len() - 1].to_vec())),
            Err(BootstrapError::Io(_))
        ));
        let oversized = (MAX_BOOTSTRAP_BYTES as u32 + 1).to_be_bytes().to_vec();
        assert!(matches!(
            read_bootstrap(&mut Cursor::new(oversized)),
            Err(BootstrapError::MalformedPayload)
        ));
        encoded.push(1);
        assert!(matches!(
            read_bootstrap(&mut Cursor::new(encoded)),
            Err(BootstrapError::TrailingBytes)
        ));
    }

    #[test]
    fn readiness_requires_the_same_credential_profile_and_loopback_endpoint() {
        let message = BootstrapMessage::new(WorkerId::new(), default_effective_profile()).unwrap();
        let endpoint = "127.0.0.1:43123".parse().unwrap();
        let mut encoded = Vec::new();
        write_authenticated_readiness(&mut encoded, &message, endpoint).unwrap();

        let readiness = read_authenticated_readiness(&mut Cursor::new(encoded), &message).unwrap();
        assert_eq!(readiness.worker_id(), message.worker_id());
        assert_eq!(readiness.effective_profile(), message.effective_profile());
        assert_eq!(readiness.endpoint(), endpoint);
    }

    #[test]
    fn readiness_rejects_public_or_unbound_endpoints_and_wrong_credentials() {
        let message = BootstrapMessage::new(WorkerId::new(), default_effective_profile()).unwrap();
        assert!(matches!(
            write_authenticated_readiness(
                &mut Vec::new(),
                &message,
                "192.0.2.10:43123".parse().unwrap()
            ),
            Err(BootstrapError::NonLoopbackEndpoint)
        ));
        assert!(matches!(
            write_authenticated_readiness(
                &mut Vec::new(),
                &message,
                "127.0.0.1:0".parse().unwrap()
            ),
            Err(BootstrapError::NonLoopbackEndpoint)
        ));

        let mut encoded = Vec::new();
        write_authenticated_readiness(&mut encoded, &message, "[::1]:43123".parse().unwrap())
            .unwrap();
        let other =
            BootstrapMessage::new(message.worker_id(), default_effective_profile()).unwrap();
        assert!(matches!(
            read_authenticated_readiness(&mut Cursor::new(encoded), &other),
            Err(BootstrapError::AuthenticationFailed)
        ));
    }
}
