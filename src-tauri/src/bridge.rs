//! Typed DTOs crossing the Tauri IPC boundary.
//!
//! Keeping them in one place makes the UI/Rust contract explicit and prevents
//! drift between the Svelte `tauriBridge.ts` types and what the command
//! handlers expect.

use holo_hash::{AgentPubKey, DnaHash};
use serde::{Deserialize, Serialize};

/// Cell identifier over the wire. We keep the raw 39-byte encoded hashes
/// instead of pulling the `holo_hash` serialisation format into the UI
/// because the UI already holds the AgentPubKey as base-64 encoded bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CellIdWire {
    pub dna_hash: Vec<u8>,
    pub agent_pub_key: Vec<u8>,
}

impl CellIdWire {
    /// Decode both halves into the `holo_hash` newtypes the conductor APIs
    /// expect. Callers combine the two into a `CellId`.
    pub fn decode(&self) -> anyhow::Result<(DnaHash, AgentPubKey)> {
        // `HoloHash::try_from_raw_39` validates the 39-byte layout (3-byte prefix
        // + 32-byte hash + 4-byte DHT coords) and returns a typed hash.
        let dna = DnaHash::try_from_raw_39(self.dna_hash.clone())
            .map_err(|e| anyhow::anyhow!("decode DnaHash: {e:?}"))?;
        let agent = AgentPubKey::try_from_raw_39(self.agent_pub_key.clone())
            .map_err(|e| anyhow::anyhow!("decode AgentPubKey: {e:?}"))?;
        Ok((dna, agent))
    }
}

/// A Holochain source-chain record on the wire. We serialize via serde_json
/// so the UI side can round-trip records as opaque JSON values without
/// pulling the Holochain type definitions into TypeScript. This trades a
/// bit of size for much simpler integration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OpaqueRecord(pub serde_json::Value);

impl OpaqueRecord {
    pub fn from_record(r: holochain_zome_types::prelude::Record) -> anyhow::Result<Self> {
        Ok(Self(serde_json::to_value(r)?))
    }
    pub fn into_record(self) -> anyhow::Result<holochain_zome_types::prelude::Record> {
        Ok(serde_json::from_value(self.0)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CellIdWire decodes a valid 39-byte DnaHash / AgentPubKey pair.
    #[test]
    fn cell_id_wire_decodes_valid_pair() {
        // Reference bytes from a real DnaHash / AgentPubKey layout: 3-byte
        // prefix + 32-byte payload + 4-byte DHT loc. We use the known
        // prefixes (0x84, 0x2d, 0x24 = Dna; 0x84, 0x20, 0x24 = Agent).
        let dna = {
            let mut v = vec![0x84, 0x2d, 0x24];
            v.extend(vec![0u8; 32]);
            v.extend(vec![0u8; 4]);
            v
        };
        let agent = {
            let mut v = vec![0x84, 0x20, 0x24];
            v.extend(vec![0u8; 32]);
            v.extend(vec![0u8; 4]);
            v
        };
        let wire = CellIdWire {
            dna_hash: dna.clone(),
            agent_pub_key: agent.clone(),
        };
        let (dna_decoded, agent_decoded) = wire.decode().expect("decode");
        assert_eq!(dna_decoded.get_raw_39(), &dna[..]);
        assert_eq!(agent_decoded.get_raw_39(), &agent[..]);
    }

    /// Malformed (wrong length) bytes produce a clean error rather than a
    /// panic — handlers surface the message to the UI.
    #[test]
    fn cell_id_wire_rejects_wrong_length() {
        let wire = CellIdWire {
            dna_hash: vec![0, 1, 2],
            agent_pub_key: vec![0; 39],
        };
        let err = wire.decode().unwrap_err().to_string();
        assert!(err.contains("DnaHash"));
    }
}
