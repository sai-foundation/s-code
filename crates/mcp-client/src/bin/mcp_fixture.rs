use serde_json::{Value, json};
use std::io::{self, BufRead, Write};

fn main() {
    let oversized_no_newline = std::env::args().any(|arg| arg == "--oversized-no-newline");
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let mut stdout = io::stdout().lock();
    while let Some(line) = lines.next() {
        let Ok(line) = line else { break };
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            break;
        };
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        if oversized_no_newline {
            stdout.write_all(&vec![b'x'; 1024 * 1024 + 1]).unwrap();
            stdout.flush().unwrap();
            break;
        }
        let result = match request.get("method").and_then(Value::as_str) {
            Some("initialize") => {
                json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{},"resources":{}},"serverInfo":{"name":"fixture","version":"1"}})
            }
            Some("tools/list") => {
                json!({"tools":[
                    {"name":"echo","description":"Echo input","inputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"]}},
                    {"name":"elicit","description":"Request safe form input","inputSchema":{"type":"object"}}
                ]})
            }
            Some("tools/call") => {
                if let Some(progress_token) =
                    request["params"]["_meta"].get("progressToken").cloned()
                {
                    for (progress, message) in
                        [(25, "Preparing fixture"), (100, "Fixture complete")]
                    {
                        writeln!(
                            stdout,
                            "{}",
                            json!({
                                "jsonrpc":"2.0",
                                "method":"notifications/progress",
                                "params":{
                                    "progressToken":progress_token,
                                    "progress":progress,
                                    "total":100,
                                    "message":message
                                }
                            })
                        )
                        .unwrap();
                    }
                    stdout.flush().unwrap();
                }
                if request["params"]["name"] == "elicit" {
                    writeln!(
                        stdout,
                        "{}",
                        json!({
                            "jsonrpc": "2.0",
                            "id": "fixture-elicitation",
                            "method": "elicitation/create",
                            "params": {
                                "message": "Choose a safe display name.",
                                "requestedSchema": {
                                    "type": "object",
                                    "properties": {"display_name": {"type": "string"}},
                                    "required": ["display_name"]
                                }
                            }
                        })
                    )
                    .unwrap();
                    stdout.flush().unwrap();
                    let response = lines
                        .next()
                        .and_then(Result::ok)
                        .and_then(|line| serde_json::from_str::<Value>(&line).ok())
                        .unwrap_or(Value::Null);
                    json!({"content":[{"type":"text","text":response["result"]["content"]["display_name"]}],"isError":false})
                } else {
                    json!({"content":[{"type":"text","text":request["params"]["arguments"]["text"]}],"isError":false})
                }
            }
            Some("resources/list") => {
                json!({"resources":[{"uri":"fixture://docs/readme","name":"README","description":"Fixture documentation","mimeType":"text/markdown"}],"nextCursor":"page-2"})
            }
            Some("resources/templates/list") => {
                json!({"resourceTemplates":[{"uriTemplate":"fixture://issues/{id}","name":"Issue","description":"Issue by ID","mimeType":"application/json"}]})
            }
            Some("resources/read") => {
                json!({"contents":[{"uri":request["params"]["uri"],"mimeType":"text/markdown","text":"# Fixture resource\n\nSafe MCP content."}]})
            }
            _ => json!({}),
        };
        writeln!(
            stdout,
            "{}",
            json!({"jsonrpc":"2.0","id":id,"result":result})
        )
        .unwrap();
        stdout.flush().unwrap();
    }
}
