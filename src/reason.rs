//! Reasoning: ask the local LFM2-VL server (llama.cpp, OpenAI-compatible) for the
//! single next action, constrained to our JSON action schema.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

/// One action the model emits per turn. All fields beyond `action` are optional and
/// only meaningful for some actions — we keep it a flat, forgiving struct so a tiny
/// model's slightly-loose JSON still deserializes.
#[derive(Debug, Default, Deserialize)]
pub struct Action {
    #[serde(default)]
    pub thought: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub keys: Option<String>,
    #[serde(default)]
    pub app: Option<String>,
}

/// Ask the model for the next action. Tries a strict JSON-schema response_format first
/// (the single biggest reliability win for a small model); falls back to json_object,
/// then to an unconstrained call, so it works across llama.cpp builds.
pub fn decide(
    client: &reqwest::blocking::Client,
    url: &str,
    model: &str,
    system: &str,
    user_text: &str,
    image_b64: Option<&str>,
    constrain: bool,
) -> Result<Action> {
    let formats: Vec<Option<Value>> = if constrain {
        vec![
            Some(schema_format()),
            Some(json!({ "type": "json_object" })),
            None,
        ]
    } else {
        // Unconstrained: let a thinking model reason freely, then parse the action out.
        vec![None]
    };
    let mut last_err = None;
    for rf in formats {
        match request(client, url, model, system, user_text, image_b64, rf) {
            // A parse failure is recoverable: advance to the next (looser) response
            // format rather than aborting the whole run on the first 200 response.
            Ok(content) => match parse_action(&content) {
                Ok(action) => return Ok(action),
                Err(e) => last_err = Some(e),
            },
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("model request failed")))
}

fn request(
    client: &reqwest::blocking::Client,
    url: &str,
    model: &str,
    system: &str,
    user_text: &str,
    image_b64: Option<&str>,
    response_format: Option<Value>,
) -> Result<String> {
    let mut content = vec![json!({ "type": "text", "text": user_text })];
    if let Some(b64) = image_b64 {
        // Text BEFORE image — improves selection accuracy for vision models.
        content.push(json!({
            "type": "image_url",
            "image_url": { "url": format!("data:image/png;base64,{}", b64) }
        }));
    }
    let mut body = json!({
        "model": model,
        "temperature": 0.1,
        "max_tokens": 2048,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": content },
        ],
    });
    if let Some(rf) = response_format {
        body["response_format"] = rf;
    }

    let resp = client
        .post(format!("{}/v1/chat/completions", url.trim_end_matches('/')))
        .json(&body)
        .send()
        .context("could not reach llama-server (is it running on the model URL?)")?
        .error_for_status()
        .context("llama-server returned an error status")?;
    let v: Value = resp.json().context("invalid JSON from llama-server")?;
    let content = v["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| anyhow!("no message content in model response"))?
        .to_string();
    Ok(content)
}

/// The strict action schema (llama.cpp turns this into a GBNF grammar).
fn schema_format() -> Value {
    json!({
        "type": "json_schema",
        "json_schema": {
            "name": "pc_action",
            "strict": false,
            "schema": {
                "type": "object",
                "properties": {
                    "thought": { "type": "string" },
                    "action": {
                        "type": "string",
                        "enum": ["open_app", "click", "type", "key", "wait", "done"]
                    },
                    "id": { "type": "integer" },
                    "text": { "type": "string" },
                    "keys": { "type": "string" },
                    "app": { "type": "string" }
                },
                "required": ["action"]
            }
        }
    })
}

/// Parse the model's text into an Action, tolerating markdown fences and surrounding prose.
pub fn parse_action(text: &str) -> Result<Action> {
    let blob = extract_json(text)
        .ok_or_else(|| anyhow!("model did not return JSON. Raw output:\n{}", text.trim()))?;
    let action: Action = serde_json::from_str(&blob)
        .with_context(|| format!("could not parse action JSON: {}", blob))?;
    if action.action.is_empty() {
        return Err(anyhow!("model returned no 'action' field: {}", blob));
    }
    Ok(action)
}

/// Pull the first balanced `{...}` object out of a string.
fn extract_json(s: &str) -> Option<String> {
    let s = s
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let start = s.find('{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in s[start..].char_indices() {
        // Braces inside a JSON string value must not affect nesting depth, or a
        // payload like {"text":"press }"} truncates mid-value.
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[start..start + i + 1].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn braces_inside_string_values_survive() {
        // The regression: a '}' inside a string value must not close the object early.
        let a = parse_action(r#"{"action":"type","text":"press the } key"}"#).unwrap();
        assert_eq!(a.action, "type");
        assert_eq!(a.text.as_deref(), Some("press the } key"));

        let b = parse_action(r#"{"action":"done","text":"saved {report}.txt"}"#).unwrap();
        assert_eq!(b.text.as_deref(), Some("saved {report}.txt"));
    }

    #[test]
    fn tolerates_markdown_fences_and_prose() {
        let a =
            parse_action("Sure! Here is the action:\n```json\n{\"action\":\"key\",\"keys\":\"cmd+s\"}\n```")
                .unwrap();
        assert_eq!(a.action, "key");
        assert_eq!(a.keys.as_deref(), Some("cmd+s"));
    }

    #[test]
    fn escaped_quotes_do_not_end_the_string() {
        let a = parse_action(r#"{"action":"type","text":"say \"hi\" }now"}"#).unwrap();
        assert_eq!(a.text.as_deref(), Some(r#"say "hi" }now"#));
    }

    #[test]
    fn rejects_non_json_and_empty_action() {
        assert!(parse_action("no json here").is_err());
        assert!(parse_action(r#"{"thought":"hi"}"#).is_err());
    }
}
