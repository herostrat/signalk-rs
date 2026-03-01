//! Depth sentences: DPT, DBT, DBS, DBK
use serde_json::json;
use super::PathValue;

const FEET_TO_M: f64 = 0.304_8;
const FATHOMS_TO_M: f64 = 1.828_8;

/// DPT — Depth of Water
/// Provides: depth below transducer; optionally below keel or surface
pub fn from_dpt(dpt: &nmea::sentences::DptData) -> Vec<PathValue> {
    let mut out = Vec::new();

    let Some(depth) = dpt.water_depth else {
        return out;
    };

    out.push(PathValue::new(
        "environment.depth.belowTransducer",
        json!(depth),
    ));

    // offset: positive → transducer above waterline (depth + offset = depth below surface)
    //         negative → transducer above keel       (depth + offset = depth below keel)
    if let Some(offset) = dpt.offset {
        let adjusted = depth + offset;
        if offset >= 0.0 {
            out.push(PathValue::new(
                "environment.depth.belowSurface",
                json!(adjusted),
            ));
        } else {
            out.push(PathValue::new(
                "environment.depth.belowKeel",
                json!(adjusted),
            ));
        }
    }

    out
}

/// DBT — Depth Below Transducer
/// Provides: depth in meters (prefers meters field, falls back to feet/fathoms)
pub fn from_dbt(dbt: &nmea::sentences::DbtData) -> Vec<PathValue> {
    let mut out = Vec::new();
    let depth = dbt
        .depth_meters
        .or_else(|| dbt.depth_feet.map(|f| f * FEET_TO_M))
        .or_else(|| dbt.depth_fathoms.map(|f| f * FATHOMS_TO_M));
    if let Some(d) = depth {
        out.push(PathValue::new(
            "environment.depth.belowTransducer",
            json!(d),
        ));
    }
    out
}

/// DBS — Depth Below Surface
/// Provides: depth below surface in meters
pub fn from_dbs(dbs: &nmea::sentences::DbsData) -> Vec<PathValue> {
    let mut out = Vec::new();
    let depth = dbs
        .water_depth_meters
        .map(|m| m as f64)
        .or_else(|| dbs.water_depth_feet.map(|f| f as f64 * FEET_TO_M))
        .or_else(|| dbs.water_depth_fathoms.map(|f| f as f64 * FATHOMS_TO_M));
    if let Some(d) = depth {
        out.push(PathValue::new("environment.depth.belowSurface", json!(d)));
    }
    out
}

/// DBK — Depth Below Keel
/// Provides: depth below keel in meters
pub fn from_dbk(dbk: &nmea::sentences::DbkData) -> Vec<PathValue> {
    let mut out = Vec::new();
    let depth = dbk
        .depth_meters
        .or_else(|| dbk.depth_feet.map(|f| f * FEET_TO_M))
        .or_else(|| dbk.depth_fathoms.map(|f| f * FATHOMS_TO_M));
    if let Some(d) = depth {
        out.push(PathValue::new("environment.depth.belowKeel", json!(d)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpt_below_transducer() {
        let dpt = nmea::sentences::DptData {
            water_depth: Some(15.3),
            offset: None,
            max_range_scale: None,
        };
        let values = from_dpt(&dpt);
        let depth = values
            .iter()
            .find(|p| p.path == "environment.depth.belowTransducer")
            .unwrap();
        assert!((depth.value.as_f64().unwrap() - 15.3).abs() < 1e-6);
    }

    #[test]
    fn dpt_below_keel_from_negative_offset() {
        let dpt = nmea::sentences::DptData {
            water_depth: Some(15.3),
            offset: Some(-1.5), // transducer is 1.5 m above keel
            max_range_scale: None,
        };
        let values = from_dpt(&dpt);
        let keel = values
            .iter()
            .find(|p| p.path == "environment.depth.belowKeel")
            .unwrap();
        assert!((keel.value.as_f64().unwrap() - 13.8).abs() < 1e-6);
    }

    #[test]
    fn dbt_meters_preferred() {
        let dbt = nmea::sentences::DbtData {
            depth_feet: Some(49.2),
            depth_meters: Some(15.0),
            depth_fathoms: Some(8.2),
        };
        let values = from_dbt(&dbt);
        assert_eq!(values.len(), 1);
        assert!((values[0].value.as_f64().unwrap() - 15.0).abs() < 1e-6);
    }

    #[test]
    fn dbt_feet_fallback() {
        let dbt = nmea::sentences::DbtData {
            depth_feet: Some(49.2126),
            depth_meters: None,
            depth_fathoms: None,
        };
        let values = from_dbt(&dbt);
        assert!((values[0].value.as_f64().unwrap() - 49.2126 * FEET_TO_M).abs() < 1e-3);
    }

    #[test]
    fn dbs_depth_below_surface() {
        let dbs = nmea::sentences::DbsData {
            water_depth_feet: None,
            water_depth_meters: Some(20.0),
            water_depth_fathoms: None,
        };
        let values = from_dbs(&dbs);
        assert_eq!(values[0].path, "environment.depth.belowSurface");
        assert!((values[0].value.as_f64().unwrap() - 20.0).abs() < 1e-6);
    }

    #[test]
    fn dbk_depth_below_keel() {
        let dbk = nmea::sentences::DbkData {
            depth_feet: None,
            depth_meters: Some(3.5),
            depth_fathoms: None,
        };
        let values = from_dbk(&dbk);
        assert_eq!(values[0].path, "environment.depth.belowKeel");
        assert!((values[0].value.as_f64().unwrap() - 3.5).abs() < 1e-6);
    }
}
