//! Paylaşılan JSON Schema doğrulama (`schemas/*.schema.json`).

use std::sync::OnceLock;

use jsonschema::Validator;
use serde_json::Value;

macro_rules! schema {
    ($name:ident, $path:literal) => {
        fn $name() -> &'static Validator {
            static V: OnceLock<Validator> = OnceLock::new();
            V.get_or_init(|| {
                let raw: Value = serde_json::from_str(include_str!(concat!("../schemas/", $path)))
                    .unwrap_or_else(|err| panic!("schema parse {}: {err}", $path));
                Validator::new(&raw).unwrap_or_else(|err| panic!("schema compile {}: {err}", $path))
            })
        }
    };
}

schema!(experience_validator, "experience.schema.json");
schema!(task_validator, "task.schema.json");
schema!(mcp_search_validator, "mcp_search_experience.schema.json");
schema!(mcp_record_validator, "mcp_record_experience.schema.json");
schema!(mcp_dispatch_validator, "mcp_dispatch_task.schema.json");
schema!(mcp_status_validator, "mcp_status.schema.json");

#[derive(Debug, Clone, Copy)]
pub enum SchemaKind {
    Experience,
    Task,
    McpSearch,
    McpRecord,
    McpDispatch,
    McpStatus,
}

/// `instance` verilen şemaya uymuyorsa insan-okur hata metni döner.
pub fn validate(kind: SchemaKind, instance: &Value) -> Result<(), String> {
    let validator = match kind {
        SchemaKind::Experience => experience_validator(),
        SchemaKind::Task => task_validator(),
        SchemaKind::McpSearch => mcp_search_validator(),
        SchemaKind::McpRecord => mcp_record_validator(),
        SchemaKind::McpDispatch => mcp_dispatch_validator(),
        SchemaKind::McpStatus => mcp_status_validator(),
    };
    let Ok(()) = validator.validate(instance) else {
        let errors: Vec<String> = validator
            .iter_errors(instance)
            .map(|err| format!("{}: {}", err.instance_path, err))
            .collect();
        return Err(format!(
            "şema doğrulama başarısız ({kind:?}): {}",
            errors.join("; ")
        ));
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_extra_mcp_search_fields() {
        let bad = json!({ "query": "nats", "hack": true });
        let err = validate(SchemaKind::McpSearch, &bad).unwrap_err();
        assert!(err.contains("şema") || err.contains("additional") || err.contains("hack"));
    }

    #[test]
    fn accepts_valid_experience() {
        let good = json!({
            "id": "11111111-1111-4111-8111-111111111111",
            "type": "experience",
            "agent": "cursor",
            "project_id": "demo",
            "adr_summary": "use pool",
            "outcome": "success",
            "created_at": "2026-09-18T12:00:00.000Z"
        });
        validate(SchemaKind::Experience, &good).expect("valid experience");
    }

    #[test]
    fn rejects_freeform_experience() {
        let bad = json!({
            "id": "11111111-1111-4111-8111-111111111111",
            "type": "experience",
            "agent": "cursor",
            "project_id": "demo",
            "adr_summary": "use pool",
            "outcome": "success",
            "created_at": "2026-09-18T12:00:00.000Z",
            "secret_blob": "nope"
        });
        assert!(validate(SchemaKind::Experience, &bad).is_err());
    }
}
