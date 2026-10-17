use serde_json::{Value, json};

fn reply(state: &Value, message: &str) -> Value {
    json!({"state": state, "message": message, "commands": [], "requests": []})
}

/// The guest proposes a typed edit; the host owns handles, authority, validation and Undo.
pub(crate) fn handle(request: &Value) -> Value {
    let state = request.get("state").unwrap_or(&Value::Null);
    match request.get("event").and_then(Value::as_str) {
        Some("organize") => {
            let values = request.get("values").unwrap_or(&Value::Null);
            let prefix = values.get("prefix").and_then(Value::as_str).unwrap_or("Organized");
            if !["Organized", "Review", "Export"].contains(&prefix) {
                return reply(state, "Choose a declared name prefix.");
            }
            let selected = values.get("selectedOnly").and_then(Value::as_bool).unwrap_or(false);
            json!({"state":{"pendingAction":"read", "prefix":prefix, "selectedOnly":selected},
                "message":"Reading layers…", "requests":[{"id":"layers", "method":"documents.layers.read", "params":{"limit":128}}]})
        }
        Some("host_response") => {
            let action = state.get("pendingAction").and_then(Value::as_str);
            let expected_id = if action == Some("edit") { "edit" } else { "layers" };
            let response = request
                .get("responses")
                .and_then(Value::as_array)
                .and_then(|items| items.iter().find(|item| item.get("id").and_then(Value::as_str) == Some(expected_id)));
            let Some(response) = response else { return reply(state, "The host response is missing. No layer changes requested.") };
            if response.get("error").is_some_and(|error| !error.is_null()) {
                return reply(&Value::Null, "PhotoCraft rejected the request. No new layer changes were requested; retry against the current document.");
            }
            if action == Some("edit") {
                return reply(&Value::Null, "Layer names organized. Use Edit > Undo to reverse the complete operation.");
            }
            if action != Some("read") {
                return reply(state, "No layer read is pending.");
            }
            let Some(result) = response.get("result") else { return reply(state, "The host returned no layer snapshot.") };
            let Some(document) = result.get("document").and_then(Value::as_str) else { return reply(&Value::Null, "Open an image before organizing layers.") };
            let Some(revision) = result.get("revision").and_then(Value::as_u64) else { return reply(state, "The layer snapshot has no revision.") };
            let Some(layers) = result.get("layers").and_then(Value::as_array) else { return reply(state, "The host returned an invalid layer snapshot.") };
            if layers.len() > 128 || result.get("nextCursor").is_some_and(|cursor| !cursor.is_null()) {
                return reply(&Value::Null, "This example handles at most 128 layers. No layers changed.");
            }
            let Some(prefix) = state.get("prefix").and_then(Value::as_str).filter(|prefix| ["Organized", "Review", "Export"].contains(prefix)) else {
                return reply(state, "The pending name prefix is invalid.");
            };
            let selected = state.get("selectedOnly").and_then(Value::as_bool).unwrap_or(false);
            let mut edits = Vec::new();
            for layer in layers {
                if selected && !layer.get("selected").and_then(Value::as_bool).unwrap_or(false) {
                    continue;
                }
                let Some(handle) = layer.get("handle").and_then(Value::as_str) else {
                    return reply(state, "The snapshot contains a layer without a handle. No layers changed.");
                };
                edits.push(json!({"layer":handle,"name":format!("{prefix} {:02}", edits.len().saturating_add(1))}));
            }
            if edits.is_empty() {
                return reply(&Value::Null, "No matching layers. No layers changed.");
            }
            json!({"state":null, "message":"Layer names organized. Use Edit > Undo to restore them.", "requests":[
                {"id":"edit", "method":"documents.layers.edit", "params":{"document":document,"expectedRevision":revision,"edits":edits}}
            ]})
        }
        _ => reply(state, "Choose Organize layer names to start."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(state: Value) -> Value {
        json!({"event":"host_response","state":state,"responses":[{"id":"layers","result":{
            "document":"opaque-document","revision":7,"layers":[
                {"handle":"opaque-first","name":"A","selected":true},
                {"handle":"opaque-second","name":"B","selected":false}
            ],"nextCursor":null
        }}]})
    }

    #[test]
    fn several_layers_become_one_atomic_request_using_only_scoped_handles() {
        let read = handle(&json!({"event":"organize","values":{},"state":null}));
        assert_eq!(read["requests"][0]["method"], "documents.layers.read");
        let edit = handle(&snapshot(read["state"].clone()));
        assert_eq!(edit["requests"].as_array().unwrap().len(), 1);
        assert_eq!(
            edit["requests"][0]["params"],
            json!({"document":"opaque-document","expectedRevision":7,
            "edits":[{"layer":"opaque-first","name":"Organized 01"},{"layer":"opaque-second","name":"Organized 02"}]})
        );
        assert!(edit.get("commands").is_none());
    }

    #[test]
    fn selected_only_is_plugin_logic() {
        let read = handle(&json!({"event":"organize","values":{"prefix":"Review","selectedOnly":true}}));
        let edit = handle(&snapshot(read["state"].clone()));
        assert_eq!(edit["requests"][0]["params"]["edits"], json!([{"layer":"opaque-first","name":"Review 01"}]));
    }

    #[test]
    fn incomplete_or_invalid_snapshot_never_requests_a_partial_edit() {
        let initial = handle(&json!({"event":"organize"}));
        for key in ["document", "revision", "layers"] {
            let mut request = snapshot(initial["state"].clone());
            request["responses"][0]["result"].as_object_mut().unwrap().remove(key);
            assert_eq!(handle(&request)["requests"], json!([]));
        }
        let mut paged = snapshot(initial["state"].clone());
        paged["responses"][0]["result"]["nextCursor"] = json!("second-page");
        assert_eq!(handle(&paged)["requests"], json!([]));
        let mut bad_layer = snapshot(initial["state"].clone());
        bad_layer["responses"][0]["result"]["layers"][1]["handle"] = Value::Null;
        assert_eq!(handle(&bad_layer)["requests"], json!([]));
    }

    #[test]
    fn edit_errors_and_unsolicited_responses_never_mutate() {
        for request in [
            Value::Null,
            json!({"event":"host_response","state":null}),
            json!({"event":"host_response","state":{"pendingAction":"edit"},"responses":[{"id":"edit","error":{"code":"staleRevision"}}]}),
        ] {
            assert_eq!(handle(&request)["requests"], json!([]));
        }
    }
}
