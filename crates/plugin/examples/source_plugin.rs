//! This example supplies a card source through the external plugin protocol.

use rustytavern_core::{Error, Result};
use serde_json::{Value, json};
use std::io::{BufRead, Read, Write};

fn serve() -> Result<()> {
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    loop {
        let mut line = String::new();
        let bytes = (&mut input).take(1024 * 1024 + 1).read_line(&mut line)?;
        if bytes == 0 {
            return Ok(());
        }
        if bytes > 1024 * 1024 || !line.ends_with('\n') {
            return Err(Error::new(
                "invalid_plugin_frame",
                "The host frame is invalid or too large.",
            ));
        }
        let request: Value = serde_json::from_str(&line)?;
        if request["jsonrpc"] != "2.0" {
            return Err(Error::new(
                "invalid_plugin_frame",
                "The host protocol is invalid.",
            ));
        }
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let response = match request["method"].as_str() {
            Some("initialize") => {
                json!({"jsonrpc":"2.0","id":id,"result":{"host_protocol":"1","plugin_id":request["params"]["plugin_id"],"service_families":["card.source/1"]}})
            }
            Some("card.source.fetch") => {
                let card: Value = serde_json::from_slice(include_bytes!("lumen.json"))?;
                json!({"jsonrpc":"2.0","id":id,"result":{"card":card}})
            }
            Some("$/cancelRequest") => continue,
            Some("shutdown") => return Ok(()),
            _ => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"The method does not exist."}})
            }
        };
        writeln!(output, "{}", serde_json::to_string(&response)?)?;
        output.flush()?;
    }
}
fn main() {
    if let Err(error) = serve() {
        eprintln!("{}", serde_json::to_string(&error).unwrap_or_default());
        std::process::exit(1);
    }
}
