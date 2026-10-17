use serde_json::{Value, json};

fn reply(state: &Value, message: &str) -> Value {
    json!({"state":state,"message":message,"requests":[],"commands":[]})
}

fn printable(value: &str, max: usize) -> String {
    value.chars().take(max).map(|character| if character.is_control() { ' ' } else { character }).collect()
}

pub(crate) fn handle(request: &Value) -> Value {
    let state = request.get("state").unwrap_or(&Value::Null);
    match request.get("event").and_then(Value::as_str) {
        Some("inspect") => json!({"state":{"pendingAction":"inspect"},"requests":[{"id":"layers","method":"documents.layers.read","params":{"limit":128}}]}),
        Some("host_response") if state.get("pendingAction").and_then(Value::as_str) == Some("inspect") => {
            let response = request
                .get("responses")
                .and_then(Value::as_array)
                .and_then(|items| items.iter().find(|item| item.get("id").and_then(Value::as_str) == Some("layers")));
            let Some(response) = response else { return reply(state, "The host returned no document information. Close and reopen to retry.") };
            if response.get("error").is_some_and(|error| !error.is_null()) {
                return json!({"state":null,"message":"Document information is unavailable. Close and reopen to retry.","view":{"title":"Document Inspector","rows":[]}});
            }
            let result = response.get("result").unwrap_or(&Value::Null);
            if result.get("document").is_none_or(Value::is_null) {
                return json!({"state":null,"view":{"title":"Document Inspector","rows":[{"label":"Document","value":"Open an image to inspect it."}]}});
            }
            let name = printable(result.get("name").and_then(Value::as_str).unwrap_or("Untitled"), 512);
            let width = result.get("width").and_then(Value::as_u64).unwrap_or(0);
            let height = result.get("height").and_then(Value::as_u64).unwrap_or(0);
            let layers = result.get("layers").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
            let total = result.get("totalLayers").and_then(Value::as_u64).unwrap_or(layers.len() as u64);
            let selected = layers.iter().filter(|layer| layer.get("selected").and_then(Value::as_bool) == Some(true));
            let selected_names =
                selected.filter_map(|layer| layer.get("name").and_then(Value::as_str)).take(4).map(|name| printable(name, 120)).collect::<Vec<_>>();
            let visible = layers.iter().filter(|layer| layer.get("visible").and_then(Value::as_bool) == Some(true)).count();
            let mut rows = vec![
                json!({"label":"Document","value":name}),
                json!({"label":"Dimensions","value":format!("{width} × {height} px")}),
                json!({"label":"Layers","value":total.to_string()}),
                json!({"label":"Visible in snapshot","value":visible.to_string()}),
                json!({"label":"Selected in snapshot","value":if selected_names.is_empty() {"None".into()} else {selected_names.join(", ")}}),
            ];
            if result.get("nextCursor").is_some_and(|cursor| !cursor.is_null()) {
                rows.push(json!({"label":"Snapshot scope","value":format!("First {} of {total} layers", layers.len())}));
            }
            json!({"state":null,"view":{"title":"Document Inspector","rows":rows}})
        }
        _ => reply(state, "Open the inspector to read document information."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> Value {
        json!({"event":"host_response","state":{"pendingAction":"inspect"},"responses":[{"id":"layers","result":{
            "document":"opaque-document","name":"Example","width":640,"height":480,"totalLayers":2,"layers":[
                {"handle":"opaque-first","name":"A","selected":true,"visible":true},
                {"handle":"opaque-second","name":"B","selected":false,"visible":false}],"nextCursor":null
        }}]})
    }

    #[test]
    fn activation_and_changes_request_a_read_only_snapshot() {
        let response = handle(&json!({"event":"inspect","notification":{"event":"document.changed"}}));
        assert_eq!(response["requests"], json!([{"id":"layers","method":"documents.layers.read","params":{"limit":128}}]));
        assert!(response.get("commands").is_none());
    }

    #[test]
    fn metadata_becomes_a_native_view_without_exposing_handles() {
        let response = handle(&snapshot());
        let view = response["view"].to_string();
        for value in ["Example", "640 × 480 px", "Selected in snapshot", "A"] {
            assert!(view.contains(value));
        }
        assert!(!view.contains("opaque"));
        assert!(response.get("requests").is_none());
        assert!(response.get("commands").is_none());
    }

    #[test]
    fn document_closing_and_errors_clear_previous_metadata() {
        let mut request = snapshot();
        request["responses"][0]["result"]["document"] = Value::Null;
        assert!(handle(&request)["view"].to_string().contains("Open an image"));
        request["responses"][0]["error"] = json!({"code":"permissionDenied"});
        assert_eq!(handle(&request)["view"]["rows"], json!([]));
    }

    #[test]
    fn hostile_names_fit_the_native_view_budget() {
        let mut request = snapshot();
        request["responses"][0]["result"]["name"] = json!("😀\n".repeat(1000));
        request["responses"][0]["result"]["layers"][0]["name"] = json!("\tX".repeat(1000));
        let response = handle(&request);
        for row in response["view"]["rows"].as_array().unwrap() {
            let value = row["value"].as_str().unwrap();
            assert!(value.chars().count() <= 1024);
            assert!(!value.chars().any(char::is_control));
        }
    }
}
