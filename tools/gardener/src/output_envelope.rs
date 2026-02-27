use crate::errors::GardenerError;
use crate::types::WorkerState;
use serde::{Deserialize, Serialize};

pub const START_MARKER: &str = "<<GARDENER_JSON_START>>";
pub const END_MARKER: &str = "<<GARDENER_JSON_END>>";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputEnvelope {
    pub schema_version: u32,
    pub state: WorkerState,
    pub payload: serde_json::Value,
}

pub fn parse_last_envelope(
    raw_text: &str,
    expected_state: WorkerState,
) -> Result<OutputEnvelope, GardenerError> {
    let start = raw_text
        .rfind(START_MARKER)
        .ok_or_else(|| GardenerError::OutputEnvelope("missing start marker".to_string()))?;
    let end = raw_text
        .rfind(END_MARKER)
        .ok_or_else(|| GardenerError::OutputEnvelope("missing end marker".to_string()))?;

    if end <= start {
        return Err(GardenerError::OutputEnvelope(
            "end marker appears before start marker".to_string(),
        ));
    }

    let body_start = start + START_MARKER.len();
    let body = raw_text[body_start..end].trim();

    let envelope: OutputEnvelope = serde_json::from_str(body)
        .map_err(|e| GardenerError::OutputEnvelope(format!("invalid json: {e}")))?;

    if envelope.schema_version != 1 {
        return Err(GardenerError::OutputEnvelope(
            "schema_version must be 1".to_string(),
        ));
    }

    if envelope.state != expected_state {
        return Err(GardenerError::OutputEnvelope(format!(
            "state mismatch: expected {:?}, got {:?}",
            expected_state, envelope.state
        )));
    }

    Ok(envelope)
}

