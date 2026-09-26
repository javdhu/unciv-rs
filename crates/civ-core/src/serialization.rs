//! Optional serde support shared by snapshots and native/browser facades.
//! Maps use entry arrays so generational and tuple keys remain lossless JSON.

use crate::arena::Id;
use crate::UniqueType;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

impl<T> Serialize for Id<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (self.index(), self.generation()).serialize(serializer)
    }
}

impl<'de, T> Deserialize<'de> for Id<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (index, generation) = <(u32, std::num::NonZeroU32)>::deserialize(deserializer)?;
        Ok(Id::from_snapshot_parts(index, generation))
    }
}

impl Serialize for UniqueType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.placeholder().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for UniqueType {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Self::ALL
            .iter()
            .find(|kind| kind.placeholder() == name)
            .copied()
            .ok_or_else(|| serde::de::Error::custom("unknown unique type"))
    }
}

/// [`map_as_entries`] for a map held in a [`crate::shared::Shared`].
pub mod shared_map_as_entries {
    use crate::shared::Shared;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;

    pub fn serialize<S, K, V>(map: &Shared<BTreeMap<K, V>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        K: Serialize,
        V: Serialize,
    {
        super::map_as_entries::serialize(map, serializer)
    }

    pub fn deserialize<'de, D, K, V>(deserializer: D) -> Result<Shared<BTreeMap<K, V>>, D::Error>
    where
        D: Deserializer<'de>,
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
    {
        super::map_as_entries::deserialize(deserializer).map(Shared::from)
    }
}

pub mod map_as_entries {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;

    pub fn serialize<S, K, V>(map: &BTreeMap<K, V>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        K: Serialize,
        V: Serialize,
    {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(map.len()))?;
        for entry in map {
            seq.serialize_element(&entry)?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D, K, V>(deserializer: D) -> Result<BTreeMap<K, V>, D::Error>
    where
        D: Deserializer<'de>,
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
    {
        let entries = Vec::<(K, V)>::deserialize(deserializer)?;
        let mut map = BTreeMap::new();
        for (key, value) in entries {
            if map.insert(key, value).is_some() {
                return Err(serde::de::Error::custom("duplicate snapshot map key"));
            }
        }
        Ok(map)
    }
}

/// Two levels of maps, used for directed diplomatic flags.
pub mod nested_map_as_entries {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;

    pub fn serialize<S, A, B, V>(map: &BTreeMap<A, BTreeMap<B, V>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        A: Serialize,
        B: Serialize,
        V: Serialize,
    {
        let entries: Vec<_> = map.iter().map(|(key, map)| (key, map.iter().collect::<Vec<_>>())).collect();
        entries.serialize(serializer)
    }

    pub fn deserialize<'de, D, A, B, V>(deserializer: D) -> Result<BTreeMap<A, BTreeMap<B, V>>, D::Error>
    where
        D: Deserializer<'de>,
        A: Deserialize<'de> + Ord,
        B: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
    {
        let entries = Vec::<(A, Vec<(B, V)>)>::deserialize(deserializer)?;
        let mut map = BTreeMap::new();
        for (key, entries) in entries {
            let mut inner = BTreeMap::new();
            for (key, value) in entries {
                if inner.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("duplicate snapshot flag"));
                }
            }
            if map.insert(key, inner).is_some() {
                return Err(serde::de::Error::custom("duplicate snapshot relation"));
            }
        }
        Ok(map)
    }
}
