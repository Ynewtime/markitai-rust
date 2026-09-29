//! Deployment occupancy belongs to a caller-owned runtime, never the process.
use super::{Deployment, Protocol, random_ticket};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;

// Keys contain a salted digest of resolved credentials and are deliberately
// absent from Debug, serialization, cache keys and public model identifiers.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Key([u8; 32]);

pub(crate) struct Table {
    salt: [u8; 32],
    active: Mutex<HashMap<Key, usize>>,
}

impl Table {
    pub(crate) fn new() -> Self {
        let mut salt = [0; 32];
        salt[..16].copy_from_slice(&random_ticket().to_le_bytes());
        salt[16..].copy_from_slice(&random_ticket().to_le_bytes());
        Self {
            salt,
            active: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn key(&self, entry: &Deployment) -> Key {
        let mut hash = Sha256::new();
        hash.update(self.salt);
        for field in [
            entry.explicit_id.as_deref(),
            Some(entry.group.as_str()),
            Some(entry.provider.as_str()),
            Some(entry.model.as_str()),
            Some(entry.endpoint.as_str()),
            entry.key.as_deref(),
        ] {
            match field {
                Some(value) => {
                    hash.update([1]);
                    hash.update((value.len() as u64).to_le_bytes());
                    hash.update(value.as_bytes());
                }
                None => hash.update([0]),
            }
        }
        hash.update([match entry.protocol {
            Protocol::Chat => 0,
            Protocol::Anthropic => 1,
            Protocol::Azure => 2,
        }]);
        Key(hash.finalize().into())
    }

    /// The caller must hold a global runtime permit until this lease is dropped.
    /// Only occupied keys are retained, bounding the table by runtime capacity.
    pub(crate) fn reserve<'a>(&'a self, keys: &[Key], candidates: &[usize]) -> (usize, Lease<'a>) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let selected = *candidates
            .iter()
            .min_by_key(|&&index| active.get(&keys[index]).copied().unwrap_or(0))
            .expect("routing requires an eligible deployment");
        let key = keys[selected];
        *active.entry(key).or_default() += 1;
        (selected, Lease { table: self, key })
    }
}

pub(crate) struct Lease<'a> {
    table: &'a Table,
    key: Key,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        let mut active = self
            .table
            .active
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(count) = active.get_mut(&self.key) {
            *count -= 1;
            if *count == 0 {
                active.remove(&self.key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LlmRuntime;

    #[test]
    fn occupancy_is_atomic_and_unwind_releases_without_retaining_history() {
        let table = Table::new();
        let keys = [Key([1; 32]), Key([2; 32])];
        let (first, lease) = table.reserve(&keys, &[0, 1]);
        assert_eq!(first, 0);
        let caught = std::panic::catch_unwind(|| {
            let (second, _lease) = table.reserve(&keys, &[0, 1]);
            assert_eq!(second, 1);
            panic!("authored request unwind");
        });
        assert!(caught.is_err());
        let (next, next_lease) = table.reserve(&keys, &[0, 1]);
        assert_eq!(next, 1);
        drop(next_lease);
        drop(lease);
        assert!(table.active.lock().unwrap().is_empty());
    }

    #[test]
    fn every_resolved_identity_component_separates_occupancy() {
        let table = Table::new();
        let cfg = serde_json::json!({"llm":{"model_list":[{"model_name":"default","litellm_params":{"model":"openai/test","api_base":"http://127.0.0.1:9","api_key":"authored-key"}}]}});
        let entry = super::super::deployments(&cfg, &HashMap::new())
            .unwrap()
            .remove(0);
        let original = table.key(&entry);
        let variants: Vec<_> = (0..6)
            .map(|field| {
                let mut changed = entry.clone();
                match field {
                    0 => changed.explicit_id = Some("deployment-b".into()),
                    1 => changed.endpoint.push_str("?other=1"),
                    2 => changed.key = Some("other-authored-key".into()),
                    3 => changed.model.push_str("-other"),
                    4 => changed.group = "backup".into(),
                    _ => changed.protocol = Protocol::Anthropic,
                }
                table.key(&changed)
            })
            .collect();
        for key in variants {
            assert!(key != original);
        }
        let mut weighted = entry;
        weighted.weight = 100;
        assert!(table.key(&weighted) == original);
        assert!(!format!("{:?}", LlmRuntime::new(2).unwrap()).contains("authored-key"));
    }
}
