use super::{tools::Tools, *};
use anyhow::{Context, bail};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Default)]
struct Call {
    id: String,
    name: String,
    arguments: String,
}

pub(super) async fn turn(config: &Config, turn: &Turn, tools: &mut Tools) -> Result<()> {
    let key = std::env::var(&config.auth.api_key_env)
        .with_context(|| format!("Set {} to the API key", config.auth.api_key_env))?;
    ensure!(
        !key.trim().is_empty(),
        "API key environment variable is empty"
    );
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(20))
        .build()?;
    let responses = config.api == Api::Responses;
    let mut history = vec![
        json!({"role":"system","content":prompt(turn)}),
        json!({"role":"user","content":tools.context(turn).await?}),
    ];
    history.extend(
        turn.history
            .iter()
            .map(|m| json!({"role":m.role,"content":m.content})),
    );
    let specs: Vec<_> = tools.specs()?.into_iter().map(|s| if responses {
        json!({"type":"function","name":s["name"],"description":s["description"],"parameters":s["parameters"],"strict":false})
    } else { json!({"type":"function","function":s}) }).collect();
    let url = format!(
        "{}/{}",
        config.endpoint.trim_end_matches('/'),
        if responses {
            "responses"
        } else {
            "chat/completions"
        }
    );
    for _ in 0..config.max_tool_rounds {
        tools.emit(Update::Status("Generating…".into())).await?;
        let mut body = json!({"model":config.model,"stream":true,"tools":specs});
        body[if responses { "input" } else { "messages" }] = json!(history);
        if responses {
            body["store"] = json!(false);
        }
        let response = client
            .post(&url)
            .bearer_auth(&key)
            .json(&body)
            .send()
            .await
            .context("LLM request failed")?;
        ensure!(
            response.status().is_success(),
            "LLM endpoint returned HTTP {}",
            response.status()
        );
        let mut stream = response.bytes_stream();
        let mut decoder = Sse::default();
        let mut round = Round::default();
        let mut bytes = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("LLM stream interrupted")?;
            bytes += chunk.len();
            ensure!(bytes <= 2_000_000, "LLM response exceeds stream limit");
            for data in decoder.push(&chunk)? {
                round.event(&data, responses, tools).await?;
            }
        }
        for data in decoder.finish()? {
            round.event(&data, responses, tools).await?;
        }
        ensure!(round.complete, "LLM stream ended before completion");
        ensure!(round.text.len() <= 65536, "LLM answer too long");
        if responses {
            history.extend(round.output);
        } else {
            let mut message = json!({"role":"assistant","content":round.text});
            if !round.calls.is_empty() {
                message["tool_calls"] = json!(round.calls.values().map(|c| json!({"id":c.id,"type":"function","function":{"name":c.name,"arguments":c.arguments}})).collect::<Vec<_>>());
            }
            history.push(message);
        }
        if round.calls.is_empty() {
            return Ok(());
        }
        ensure!(round.calls.len() <= 16, "Too many parallel mail tool calls");
        for call in round.calls.values() {
            ensure!(
                !call.id.is_empty() && !call.name.is_empty(),
                "Incomplete function call"
            );
            let result = match serde_json::from_str(&call.arguments) {
                Ok(args) => tools.result(&call.name, args).await,
                Err(_) => json!({"error":"Invalid JSON tool arguments"}),
            }
            .to_string();
            history.push(if responses {
                json!({"type":"function_call_output","call_id":call.id,"output":result})
            } else {
                json!({"role":"tool","tool_call_id":call.id,"content":result})
            });
        }
        ensure!(
            serde_json::to_vec(&history)?.len() <= 300_000,
            "Conversation tool context exhausted; narrow the question"
        );
    }
    bail!("Assistant reached llm.max_tool_rounds; narrow the question or increase the limit")
}

#[derive(Default)]
struct Round {
    text: String,
    calls: BTreeMap<usize, Call>,
    output: Vec<Value>,
    complete: bool,
}
impl Round {
    async fn event(&mut self, data: &str, responses: bool, tools: &Tools) -> Result<()> {
        if data == "[DONE]" {
            return Ok(());
        }
        let event: Value = serde_json::from_str(data).context("Invalid JSON in LLM stream")?;
        ensure!(event.get("error").is_none(), "LLM stream reported an error");
        let delta;
        if responses {
            match event["type"].as_str().unwrap_or_default() {
                "response.output_text.delta" => delta = event["delta"].as_str().unwrap_or_default(),
                "response.completed" => {
                    self.complete = true;
                    self.output = event["response"]["output"]
                        .as_array()
                        .context("Responses completion missing output")?
                        .clone();
                    for (i, item) in self.output.iter().enumerate() {
                        if item["type"] == "function_call" {
                            self.calls.insert(
                                i,
                                Call {
                                    id: item["call_id"].as_str().unwrap_or_default().into(),
                                    name: item["name"].as_str().unwrap_or_default().into(),
                                    arguments: item["arguments"]
                                        .as_str()
                                        .unwrap_or_default()
                                        .into(),
                                },
                            );
                        }
                    }
                    return Ok(());
                }
                "response.failed" | "response.incomplete" | "error" => {
                    bail!("Responses generation failed or was incomplete")
                }
                _ => return Ok(()),
            }
        } else {
            let Some(choice) = event["choices"].as_array().and_then(|c| c.first()) else {
                return Ok(());
            };
            if let Some(reason) = choice["finish_reason"].as_str() {
                ensure!(
                    ["stop", "tool_calls"].contains(&reason),
                    "LLM generation ended with {reason}"
                );
                self.complete = true;
            }
            if let Some(calls) = choice["delta"]["tool_calls"].as_array() {
                for c in calls {
                    let index = c["index"].as_u64().context("Missing tool call index")? as usize;
                    ensure!(index < 16, "Too many function calls");
                    let call = self.calls.entry(index).or_default();
                    if let Some(id) = c["id"].as_str() {
                        call.id.push_str(id);
                    }
                    if let Some(name) = c["function"]["name"].as_str() {
                        call.name.push_str(name);
                    }
                    if let Some(args) = c["function"]["arguments"].as_str() {
                        call.arguments.push_str(args);
                    }
                }
            }
            delta = choice["delta"]["content"].as_str().unwrap_or_default();
        }
        self.text.push_str(delta);
        ensure!(self.text.len() <= 65536, "LLM answer too long");
        if !delta.is_empty() {
            tools.emit(Update::Text(delta.into())).await?;
        }
        Ok(())
    }
}

/// Decode bytes only at complete SSE boundaries, including split UTF-8/CRLF.
#[derive(Default)]
struct Sse {
    pending: Vec<u8>,
    data: Vec<String>,
}
impl Sse {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>> {
        self.pending.extend_from_slice(bytes);
        ensure!(self.pending.len() <= 2_000_000, "Oversized SSE frame");
        let mut events = vec![];
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<_> = self.pending.drain(..=end).collect();
            self.line(
                std::str::from_utf8(&line)?.trim_end_matches(['\r', '\n']),
                &mut events,
            );
        }
        Ok(events)
    }
    fn line(&mut self, line: &str, events: &mut Vec<String>) {
        if line.is_empty() {
            if !self.data.is_empty() {
                events.push(self.data.join("\n"));
                self.data.clear();
            }
        } else if let Some(data) = line.strip_prefix("data:") {
            self.data
                .push(data.strip_prefix(' ').unwrap_or(data).into());
        }
    }
    fn finish(&mut self) -> Result<Vec<String>> {
        let pending = std::mem::take(&mut self.pending);
        let mut events = vec![];
        if !pending.is_empty() {
            self.line(
                std::str::from_utf8(&pending)?.trim_end_matches('\r'),
                &mut events,
            );
        }
        self.line("", &mut events);
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sse_survives_every_byte_boundary_and_multiline_data() {
        let wire = "event: delta\r\ndata: {\"text\":\"été 📬\"}\r\n\r\n: heartbeat\ndata: one\ndata: two\n\n".as_bytes();
        let mut sse = Sse::default();
        let mut events = vec![];
        for b in wire {
            events.extend(sse.push(&[*b]).unwrap());
        }
        events.extend(sse.finish().unwrap());
        assert_eq!(events, vec!["{\"text\":\"été 📬\"}", "one\ntwo"]);
    }
}
