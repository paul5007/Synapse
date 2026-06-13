//! `CF_AGENT_TRANSCRIPTS` key codec (#900).
//!
//! Keys are `spawn_id bytes || 0x00 || line_no (8 bytes BE)`. Spawn ids are
//! strictly `agent-spawn-` plus ASCII alphanumerics/dashes (enforced at
//! ingest), so the `0x00` separator can never appear inside an id and the
//! key space is unambiguous. Rows for one spawn iterate contiguously in
//! source-line order under a prefix scan, and re-ingesting a line always
//! lands on the same key — ingestion is idempotent by construction.
//!
//! Every producer and consumer must encode/decode through this module so a
//! malformed key is a structured error, never a silent skip.

use crate::{StorageError, StorageResult, cf};

/// Separator between the spawn id and the line number.
const KEY_SEPARATOR: u8 = 0x00;

/// Encodes a `CF_AGENT_TRANSCRIPTS` row key.
#[must_use]
pub fn agent_transcript_key(spawn_id: &str, line_no: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(spawn_id.len() + 1 + 8);
    key.extend_from_slice(spawn_id.as_bytes());
    key.push(KEY_SEPARATOR);
    key.extend_from_slice(&line_no.to_be_bytes());
    key
}

/// Encodes the prefix that scans all rows of one spawn.
#[must_use]
pub fn agent_transcript_spawn_prefix(spawn_id: &str) -> Vec<u8> {
    let mut prefix = Vec::with_capacity(spawn_id.len() + 1);
    prefix.extend_from_slice(spawn_id.as_bytes());
    prefix.push(KEY_SEPARATOR);
    prefix
}

/// Decodes a `CF_AGENT_TRANSCRIPTS` row key into `(spawn_id, line_no)`.
///
/// # Errors
///
/// Returns [`StorageError::ReadFailed`] when the key lacks the separator,
/// the spawn id bytes are not UTF-8, or the line-number suffix is not
/// exactly 8 bytes.
pub fn decode_agent_transcript_key(key: &[u8]) -> StorageResult<(String, u64)> {
    let invalid = |detail: String| StorageError::ReadFailed {
        cf_name: cf::CF_AGENT_TRANSCRIPTS.to_owned(),
        detail,
    };
    let separator_at = key
        .iter()
        .position(|byte| *byte == KEY_SEPARATOR)
        .ok_or_else(|| {
            invalid("AGENT_TRANSCRIPT_KEY_INVALID: missing 0x00 separator".to_owned())
        })?;
    let (id_bytes, rest) = key.split_at(separator_at);
    let line_bytes = &rest[1..];
    if line_bytes.len() != 8 {
        return Err(invalid(format!(
            "AGENT_TRANSCRIPT_KEY_INVALID: expected 8 line-number bytes after separator, got {}",
            line_bytes.len()
        )));
    }
    let spawn_id = std::str::from_utf8(id_bytes)
        .map_err(|_e| {
            invalid("AGENT_TRANSCRIPT_KEY_INVALID: spawn id bytes are not UTF-8".to_owned())
        })?
        .to_owned();
    let line_no = u64::from_be_bytes(line_bytes.try_into().map_err(|_e| {
        invalid("AGENT_TRANSCRIPT_KEY_INVALID: line-number bytes unreadable".to_owned())
    })?);
    Ok((spawn_id, line_no))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn keys_order_by_line_number_within_a_spawn() {
        let first = agent_transcript_key("agent-spawn-a", 1);
        let second = agent_transcript_key("agent-spawn-a", 2);
        let tenth = agent_transcript_key("agent-spawn-a", 10);
        assert!(first < second, "line order must hold");
        assert!(second < tenth, "BE encoding must order numerically");
    }

    #[test]
    fn prefix_scans_cannot_bleed_across_spawns() {
        // `agent-spawn-a` is a strict prefix of `agent-spawn-ab`; the 0x00
        // separator must keep their row ranges disjoint.
        let prefix = agent_transcript_spawn_prefix("agent-spawn-a");
        let own_row = agent_transcript_key("agent-spawn-a", u64::MAX);
        let other_row = agent_transcript_key("agent-spawn-ab", 1);
        assert!(own_row.starts_with(&prefix));
        assert!(!other_row.starts_with(&prefix));
    }

    #[test]
    fn decode_rejects_malformed_keys() {
        let error = decode_agent_transcript_key(b"no-separator-here")
            .expect_err("missing separator must be rejected");
        assert!(
            error.to_string().contains("AGENT_TRANSCRIPT_KEY_INVALID"),
            "{error}"
        );
        let error = decode_agent_transcript_key(b"agent-spawn-a\x00short")
            .expect_err("short line bytes must be rejected");
        assert!(error.to_string().contains("8 line-number bytes"), "{error}");
    }

    proptest! {
        #[test]
        fn key_roundtrip(suffix in "[a-z0-9-]{1,40}", line_no in 1_u64..) {
            let spawn_id = format!("agent-spawn-{suffix}");
            let key = agent_transcript_key(&spawn_id, line_no);
            let (decoded_id, decoded_line) =
                decode_agent_transcript_key(&key).expect("roundtrip");
            prop_assert_eq!(decoded_id, spawn_id);
            prop_assert_eq!(decoded_line, line_no);
        }
    }
}
