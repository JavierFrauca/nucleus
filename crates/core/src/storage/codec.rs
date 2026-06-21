//! bincode 2.x (serde mode) value encoding for redb.
//!
//! redb stores opaque `&[u8]` values; every entity is (de)serialised here with a
//! single shared configuration so on-disk layout stays consistent.

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::Result;

fn config() -> bincode::config::Configuration {
    bincode::config::standard()
}

/// Serialise a value to bytes for storage.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    Ok(bincode::serde::encode_to_vec(value, config())?)
}

/// Deserialise a value from stored bytes.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let (value, _) = bincode::serde::decode_from_slice(bytes, config())?;
    Ok(value)
}
