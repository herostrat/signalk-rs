use crate::meta::Metadata;
use crate::source::SourceRef;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single leaf value in the full SignalK data model.
///
/// Each measurement path stores the current value, its source reference,
/// a timestamp, and optional metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalKValue {
    /// The actual measurement — scalar or structured object
    pub value: serde_json::Value,

    /// Reference to the source in the top-level sources registry
    #[serde(rename = "$source")]
    pub source: SourceRef,

    /// When this value was recorded
    pub timestamp: DateTime<Utc>,

    /// Optional metadata (units, description, alarm zones, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<Metadata>,
}

impl SignalKValue {
    pub fn new(value: serde_json::Value, source: SourceRef, timestamp: DateTime<Utc>) -> Self {
        SignalKValue {
            value,
            source,
            timestamp,
            meta: None,
        }
    }

    pub fn with_meta(mut self, meta: Metadata) -> Self {
        self.meta = Some(meta);
        self
    }
}

/// Vessel data — a flat map from dot-path to its current value.
///
/// Paths are leaf-node keys, e.g. "navigation.speedOverGround".
/// The map is serialized as a nested JSON object for REST API responses
/// and deserialized back to the flat representation.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VesselData {
    pub uuid: Option<String>,
    pub name: Option<String>,
    pub mmsi: Option<String>,

    /// Flat path → value store.
    /// e.g. `"navigation.speedOverGround"` → `SignalKValue { value: 3.85, ... }`
    pub values: HashMap<String, SignalKValue>,
}

// ── Serialization helpers ──────────────────────────────────────────────────────

/// Inserts `value` at the nested path described by `parts` within `map`.
///
/// e.g. `parts = ["navigation", "speedOverGround"]` creates
/// `{"navigation": {"speedOverGround": value}}`.
fn insert_nested(
    map: &mut serde_json::Map<String, serde_json::Value>,
    parts: &[&str],
    value: serde_json::Value,
) {
    let Some((head, tail)) = parts.split_first() else {
        return;
    };
    if tail.is_empty() {
        map.insert((*head).to_string(), value);
        return;
    }
    let entry = map
        .entry((*head).to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if let serde_json::Value::Object(nested) = entry {
        insert_nested(nested, tail, value);
    }
}

impl serde::Serialize for VesselData {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serde_json::Map::new();

        if let Some(ref v) = self.uuid {
            map.insert("uuid".to_string(), serde_json::Value::String(v.clone()));
        }
        if let Some(ref v) = self.name {
            map.insert("name".to_string(), serde_json::Value::String(v.clone()));
        }
        if let Some(ref v) = self.mmsi {
            map.insert("mmsi".to_string(), serde_json::Value::String(v.clone()));
        }

        for (flat_key, sk_value) in &self.values {
            let parts: Vec<&str> = flat_key.split('.').collect();
            let leaf = serde_json::to_value(sk_value).map_err(serde::ser::Error::custom)?;
            insert_nested(&mut map, &parts, leaf);
        }

        map.serialize(serializer)
    }
}

// ── Deserialization helpers ────────────────────────────────────────────────────

/// Recursively flattens a nested JSON value into `map` using dot-notation keys.
///
/// A JSON object is treated as a `SignalKValue` leaf when it contains all three
/// of `"value"`, `"$source"`, and `"timestamp"`.  Everything else is recursed.
fn flatten_into(
    prefix: &str,
    json: &serde_json::Value,
    map: &mut HashMap<String, SignalKValue>,
) -> Result<(), String> {
    let serde_json::Value::Object(obj) = json else {
        return Ok(());
    };
    // Heuristic: SignalKValue leaves always carry these three fields.
    if obj.contains_key("value") && obj.contains_key("$source") && obj.contains_key("timestamp") {
        let sk: SignalKValue = serde_json::from_value(json.clone()).map_err(|e| e.to_string())?;
        map.insert(prefix.to_string(), sk);
    } else {
        for (key, val) in obj {
            let child_prefix = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            flatten_into(&child_prefix, val, map)?;
        }
    }
    Ok(())
}

impl<'de> serde::Deserialize<'de> for VesselData {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;

        let mut vessel = VesselData::default();

        for (key, val) in raw {
            match key.as_str() {
                "uuid" => vessel.uuid = val.as_str().map(str::to_owned),
                "name" => vessel.name = val.as_str().map(str::to_owned),
                "mmsi" => vessel.mmsi = val.as_str().map(str::to_owned),
                other => {
                    flatten_into(other, &val, &mut vessel.values)
                        .map_err(serde::de::Error::custom)?;
                }
            }
        }

        Ok(vessel)
    }
}

/// The complete SignalK data model (full format).
///
/// Spec: https://signalk.org/specification/1.7.0/doc/data_model.html
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FullModel {
    pub version: String,

    /// URI of the local vessel, e.g. "urn:mrn:signalk:uuid:..."
    #[serde(rename = "self")]
    pub self_uri: String,

    pub vessels: HashMap<String, VesselData>,

    /// Sources registry — maps source ref strings to source info
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub sources: HashMap<String, serde_json::Value>,
}

impl FullModel {
    pub fn new(self_uri: impl Into<String>) -> Self {
        FullModel {
            version: crate::SIGNALK_VERSION.to_string(),
            self_uri: self_uri.into(),
            vessels: HashMap::new(),
            sources: HashMap::new(),
        }
    }
}

/// REST API: discovery response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryResponse {
    pub endpoints: HashMap<String, EndpointInfo>,
    pub server: ServerInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointInfo {
    pub version: String,
    #[serde(rename = "signalk-http")]
    pub signalk_http: String,
    #[serde(rename = "signalk-ws")]
    pub signalk_ws: String,
    #[serde(rename = "signalk-tcp", skip_serializing_if = "Option::is_none")]
    pub signalk_tcp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub id: String,
    pub version: String,
}

/// REST API: auth endpoints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    pub token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_to_live: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_value(v: f64, source: &str) -> SignalKValue {
        SignalKValue::new(
            serde_json::Value::Number(serde_json::Number::from_f64(v).unwrap()),
            crate::source::SourceRef::new(source),
            "2024-01-01T00:00:00Z".parse().unwrap(),
        )
    }

    #[test]
    fn vessel_data_flat_to_nested_serialization() {
        let mut vessel = VesselData {
            uuid: Some("urn:mrn:signalk:uuid:test".to_string()),
            ..Default::default()
        };
        vessel.values.insert(
            "navigation.speedOverGround".to_string(),
            make_value(3.85, "gps.0"),
        );
        vessel.values.insert(
            "navigation.courseOverGroundTrue".to_string(),
            make_value(1.23, "gps.0"),
        );
        vessel.values.insert(
            "environment.depth.belowKeel".to_string(),
            make_value(12.5, "depth.0"),
        );

        let json: serde_json::Value = serde_json::to_value(&vessel).unwrap();

        assert_eq!(json["uuid"], "urn:mrn:signalk:uuid:test");
        assert_eq!(json["navigation"]["speedOverGround"]["value"], 3.85);
        assert_eq!(json["navigation"]["courseOverGroundTrue"]["value"], 1.23);
        assert_eq!(json["environment"]["depth"]["belowKeel"]["value"], 12.5);
        assert_eq!(json["navigation"]["speedOverGround"]["$source"], "gps.0");
        // Flat key must NOT appear at the top level
        assert!(json.get("navigation.speedOverGround").is_none());
    }

    #[test]
    fn vessel_data_roundtrip() {
        let mut vessel = VesselData {
            uuid: Some("urn:mrn:signalk:uuid:test".to_string()),
            name: Some("My Boat".to_string()),
            mmsi: None,
            values: HashMap::new(),
        };
        vessel.values.insert(
            "navigation.speedOverGround".to_string(),
            make_value(3.85, "gps.0"),
        );
        vessel.values.insert(
            "environment.depth.belowKeel".to_string(),
            make_value(12.5, "depth.0"),
        );

        let json = serde_json::to_string(&vessel).unwrap();
        let back: VesselData = serde_json::from_str(&json).unwrap();

        assert_eq!(back.uuid, vessel.uuid);
        assert_eq!(back.name, vessel.name);
        assert_eq!(back.mmsi, vessel.mmsi);
        assert!(back.values.contains_key("navigation.speedOverGround"));
        assert!(back.values.contains_key("environment.depth.belowKeel"));
        let sog = &back.values["navigation.speedOverGround"];
        assert_eq!(sog.value, serde_json::json!(3.85));
        assert_eq!(sog.source, crate::source::SourceRef::new("gps.0"));
    }

    #[test]
    fn full_model_roundtrip() {
        let model = FullModel::new("urn:mrn:signalk:uuid:12345678-1234-1234-1234-123456789012");
        let json = serde_json::to_string(&model).unwrap();
        let back: FullModel = serde_json::from_str(&json).unwrap();
        assert_eq!(model.self_uri, back.self_uri);
        assert_eq!(back.version, crate::SIGNALK_VERSION);
    }

    #[test]
    fn discovery_response_serializes_correctly() {
        let resp = DiscoveryResponse {
            endpoints: {
                let mut m = HashMap::new();
                m.insert(
                    "v1".to_string(),
                    EndpointInfo {
                        version: "1.7.0".to_string(),
                        signalk_http: "http://localhost:3000/signalk/v1".to_string(),
                        signalk_ws: "ws://localhost:3000/signalk/v1/stream".to_string(),
                        signalk_tcp: None,
                    },
                );
                m
            },
            server: ServerInfo {
                id: "signalk-rs".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };
        let json: serde_json::Value = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            json["endpoints"]["v1"]["signalk-http"],
            "http://localhost:3000/signalk/v1"
        );
        assert!(
            !json["endpoints"]["v1"]
                .as_object()
                .unwrap()
                .contains_key("signalk-tcp")
        );
    }
}
