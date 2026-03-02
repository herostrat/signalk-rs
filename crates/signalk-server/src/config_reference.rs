/// Config reference generator — converts JSON Schema to commented TOML.
///
/// This module works with any JSON Schema, regardless of whether it was generated
/// by `schemars` (Rust Tier 1), a Node.js bridge plugin, or a standalone plugin.
/// The output is a fully commented TOML config section that can be copy-pasted.
use serde_json::Value;

/// Generate a commented TOML reference for a single plugin from its JSON Schema.
///
/// Returns lines like:
/// ```toml
/// # Minimum seconds between recorded points per vessel.
/// # Type: integer | Default: 5
/// min_interval_secs = 5
/// ```
pub fn schema_to_toml(schema: &Value) -> String {
    let mut out = String::new();
    if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
        let required = schema
            .get("required")
            .and_then(|r| r.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();

        for (key, prop) in props {
            emit_property(&mut out, key, prop, 0, required.contains(&key.as_str()));
        }
    }
    out
}

/// Generate a complete server config reference TOML.
///
/// Includes the server-level schema (ServerConfig) and all registered plugin schemas.
/// `plugin_schemas` is a list of `(plugin_id, plugin_name, Option<schema>)`.
pub fn generate_full_reference(
    server_schema: &Value,
    plugin_schemas: &[(String, String, Option<Value>)],
) -> String {
    let mut out = String::new();

    out.push_str(
        "# ============================================================================\n",
    );
    out.push_str("# signalk-rs — Auto-Generated Configuration Reference\n");
    out.push_str(
        "# ============================================================================\n",
    );
    out.push_str("#\n");
    out.push_str("# Generated from plugin JSON Schemas at runtime.\n");
    out.push_str("# All values shown are defaults unless marked [REQUIRED].\n");
    out.push_str(
        "# ============================================================================\n\n",
    );

    // Server-level config sections
    if let Some(props) = server_schema.get("properties").and_then(|p| p.as_object()) {
        let section_order = [
            "data_dir",
            "modules_dir",
            "server",
            "vessel",
            "auth",
            "internal",
            "history",
            "source_priorities",
            "source_ttls",
        ];

        // Top-level scalar fields first
        for key in &["data_dir", "modules_dir"] {
            if let Some(prop) = props.get(*key) {
                emit_property(&mut out, key, prop, 0, false);
            }
        }
        out.push('\n');

        // Sections
        for section_name in &section_order[2..] {
            if let Some(prop) = props.get(*section_name) {
                let type_str = prop.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if type_str == "object" {
                    // Emit description as comment
                    if let Some(desc) = prop.get("description").and_then(|d| d.as_str()) {
                        for line in desc.lines().take(2) {
                            out.push_str(&format!("# {line}\n"));
                        }
                    }
                    out.push_str(&format!("[{section_name}]\n"));
                    if let Some(inner_props) = prop.get("properties").and_then(|p| p.as_object()) {
                        let inner_required = prop
                            .get("required")
                            .and_then(|r| r.as_array())
                            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
                            .unwrap_or_default();
                        for (k, v) in inner_props {
                            emit_property(&mut out, k, v, 0, inner_required.contains(&k.as_str()));
                        }
                    }
                    out.push('\n');
                }
            }
        }
    }

    // Plugin sections
    if !plugin_schemas.is_empty() {
        out.push_str(
            "# ============================================================================\n",
        );
        out.push_str("# PLUGINS\n");
        out.push_str(
            "# ============================================================================\n\n",
        );

        for (id, name, schema) in plugin_schemas {
            out.push_str(&format!("# --- {name} ---\n"));
            out.push_str("[[plugins]]\n");
            out.push_str(&format!("id = \"{id}\"\n"));
            out.push_str("# enabled = true\n");

            match schema {
                Some(s)
                    if s.get("properties")
                        .and_then(|p| p.as_object())
                        .is_some_and(|p| !p.is_empty()) =>
                {
                    out.push_str("# config = {\n");
                    let props = s["properties"].as_object().unwrap();
                    let required = s
                        .get("required")
                        .and_then(|r| r.as_array())
                        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
                        .unwrap_or_default();
                    for (key, prop) in props {
                        emit_plugin_property(
                            &mut out,
                            key,
                            prop,
                            1,
                            required.contains(&key.as_str()),
                        );
                    }
                    out.push_str("# }\n");
                }
                _ => {
                    out.push_str("config = {}\n");
                }
            }
            out.push('\n');
        }
    }

    out
}

/// Emit a single property as commented TOML.
fn emit_property(out: &mut String, key: &str, prop: &Value, indent: usize, required: bool) {
    let prefix = "  ".repeat(indent);
    let type_str = json_type(prop);
    let default_val = format_default(prop);
    let desc = prop
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("");

    if !desc.is_empty() {
        for line in desc.lines() {
            out.push_str(&format!("{prefix}# {line}\n"));
        }
    }

    let req_tag = if required { " [REQUIRED]" } else { "" };
    if !type_str.is_empty() || !default_val.is_empty() {
        let mut meta = format!("Type: {type_str}");
        if !default_val.is_empty() {
            meta.push_str(&format!(" | Default: {default_val}"));
        }
        if !req_tag.is_empty() {
            meta.push_str(req_tag);
        }
        out.push_str(&format!("{prefix}# {meta}\n"));
    }

    let toml_val = default_to_toml(prop);
    out.push_str(&format!("{prefix}{key} = {toml_val}\n"));
}

/// Emit a plugin config property as a commented inline entry.
fn emit_plugin_property(out: &mut String, key: &str, prop: &Value, indent: usize, required: bool) {
    let prefix = "#   ".to_string() + &"  ".repeat(indent.saturating_sub(1));
    let type_str = json_type(prop);
    let desc = prop
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("");
    let default_val = format_default(prop);
    let req_tag = if required { " [REQUIRED]" } else { "" };

    let mut comment = String::new();
    if !desc.is_empty() {
        comment.push_str(desc);
    }
    if !type_str.is_empty() {
        if !comment.is_empty() {
            comment.push_str(" — ");
        }
        comment.push_str(&type_str);
    }
    if !default_val.is_empty() {
        comment.push_str(&format!(", default: {default_val}"));
    }
    comment.push_str(req_tag);

    let toml_val = default_to_toml(prop);
    out.push_str(&format!("{prefix}{key} = {toml_val}  # {comment}\n"));
}

/// Extract a human-readable type string from a JSON Schema property.
fn json_type(prop: &Value) -> String {
    if let Some(t) = prop.get("type").and_then(|t| t.as_str()) {
        if t == "object" {
            return "object".to_string();
        }
        if let Some(enum_vals) = prop.get("enum").and_then(|e| e.as_array()) {
            let vals: Vec<String> = enum_vals
                .iter()
                .filter_map(|v| v.as_str().map(|s| format!("\"{s}\"")))
                .collect();
            if !vals.is_empty() {
                return vals.join(" | ");
            }
        }
        return t.to_string();
    }
    String::new()
}

/// Format the default value for display.
fn format_default(prop: &Value) -> String {
    match prop.get("default") {
        Some(Value::String(s)) => format!("\"{s}\""),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Array(a)) if a.is_empty() => "[]".to_string(),
        Some(Value::Object(o)) if o.is_empty() => "{}".to_string(),
        Some(v) => v.to_string(),
        None => String::new(),
    }
}

/// Convert a JSON Schema property's default to a TOML-compatible value string.
fn default_to_toml(prop: &Value) -> String {
    match prop.get("default") {
        Some(Value::String(s)) => format!("\"{s}\""),
        Some(Value::Number(n)) => {
            // TOML needs a decimal point for floats
            let s = n.to_string();
            if n.is_f64() && !s.contains('.') {
                format!("{s}.0")
            } else {
                s
            }
        }
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Array(a)) if a.is_empty() => "[]".to_string(),
        Some(Value::Array(a)) => {
            let items: Vec<String> = a
                .iter()
                .map(|v| match v {
                    Value::String(s) => format!("\"{s}\""),
                    other => other.to_string(),
                })
                .collect();
            format!("[{}]", items.join(", "))
        }
        Some(Value::Object(o)) if o.is_empty() => "{}".to_string(),
        Some(Value::Null) | None => {
            // Use a type-appropriate placeholder
            match prop.get("type").and_then(|t| t.as_str()) {
                Some("string") => "\"\"".to_string(),
                Some("integer") => "0".to_string(),
                Some("number") => "0.0".to_string(),
                Some("boolean") => "false".to_string(),
                Some("array") => "[]".to_string(),
                Some("object") => "{}".to_string(),
                _ => "\"\"".to_string(),
            }
        }
        _ => "\"\"".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn simple_schema_to_toml() {
        let schema = json!({
            "type": "object",
            "properties": {
                "min_interval_secs": {
                    "type": "integer",
                    "description": "Minimum seconds between points",
                    "default": 5
                },
                "track_self": {
                    "type": "boolean",
                    "description": "Track own vessel",
                    "default": true
                }
            }
        });

        let toml = schema_to_toml(&schema);
        assert!(toml.contains("min_interval_secs = 5"));
        assert!(toml.contains("track_self = true"));
        assert!(toml.contains("# Minimum seconds between points"));
    }

    #[test]
    fn required_fields_tagged() {
        let schema = json!({
            "type": "object",
            "required": ["addr"],
            "properties": {
                "addr": {
                    "type": "string",
                    "description": "Bind address"
                }
            }
        });

        let toml = schema_to_toml(&schema);
        assert!(toml.contains("[REQUIRED]"));
    }

    #[test]
    fn plugin_section_generation() {
        let server = json!({"type": "object", "properties": {}});
        let plugins = vec![(
            "tracks".to_string(),
            "Vessel Tracks".to_string(),
            Some(json!({
                "type": "object",
                "properties": {
                    "max_age_hours": {
                        "type": "integer",
                        "description": "Max age in hours",
                        "default": 24
                    }
                }
            })),
        )];

        let out = generate_full_reference(&server, &plugins);
        assert!(out.contains("[[plugins]]"));
        assert!(out.contains("id = \"tracks\""));
        assert!(out.contains("max_age_hours"));
    }

    #[test]
    fn empty_schema_plugin() {
        let server = json!({"type": "object", "properties": {}});
        let plugins = vec![("derived-data".to_string(), "Derived Data".to_string(), None)];

        let out = generate_full_reference(&server, &plugins);
        assert!(out.contains("config = {}"));
    }

    #[test]
    fn enum_type_display() {
        let schema = json!({
            "type": "object",
            "properties": {
                "transport": {
                    "type": "string",
                    "enum": ["socketcan", "slcan", "actisense"],
                    "default": "socketcan"
                }
            }
        });

        let toml = schema_to_toml(&schema);
        assert!(toml.contains("\"socketcan\" | \"slcan\" | \"actisense\""));
    }
}
