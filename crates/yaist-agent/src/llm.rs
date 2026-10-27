//! Client for an OpenAI-compatible chat endpoint (llama.cpp's llama-server).

use std::time::Duration;

use anyhow::{anyhow, bail, Context as _, Result};
use serde_json::{json, Map, Value};

#[derive(Clone, Debug)]
pub struct LlmConfig {
    pub url: String,
    pub model: String,
}

impl LlmConfig {
    /// `YAIST_LLM_URL` (default `http://127.0.0.1:8080`) and `YAIST_LLM_MODEL`.
    pub fn from_env() -> Self {
        Self {
            url: std::env::var("YAIST_LLM_URL").unwrap_or_else(|_| "http://127.0.0.1:8080".into()).trim_end_matches('/').to_owned(),
            model: std::env::var("YAIST_LLM_MODEL").unwrap_or_else(|_| "local".into()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// `Err` holds the raw text when the model produced invalid JSON.
    pub arguments: std::result::Result<Map<String, Value>, String>,
}

pub struct Reply {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    /// The assistant message as returned, for the conversation history.
    pub message: Value,
}

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    pub config: LlmConfig,
}

impl LlmClient {
    pub fn new(config: LlmConfig) -> Self {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(180)).build().expect("HTTP client");
        Self { http, config }
    }

    /// The name of the loaded model; doubles as a health check.
    pub async fn model_name(&self) -> Result<String> {
        let v: Value =
            self.http.get(format!("{}/v1/models", self.config.url)).timeout(Duration::from_secs(3)).send().await?.error_for_status()?.json().await?;
        let id = v["data"][0]["id"].as_str().unwrap_or("unknown model");
        // "unsloth/Qwen3-4B-Instruct-2507-GGUF:Q4_K_M" -> "Qwen3-4B-Instruct-2507"
        let name = id.rsplit('/').next().unwrap_or(id);
        let name = name.split(':').next().unwrap_or(name);
        Ok(name.trim_end_matches(".gguf").trim_end_matches("-GGUF").to_owned())
    }

    pub async fn chat(&self, messages: &[Value], tools: &[Value]) -> Result<Reply> {
        self.chat_limited(messages, tools, 600).await
    }

    pub async fn chat_limited(&self, messages: &[Value], tools: &[Value], max_tokens: u32) -> Result<Reply> {
        let mut body = json!({
            "model": self.config.model,
            "messages": messages,
            "tools": tools,
            "tool_choice": "auto",
            "temperature": 0.2,
            "max_tokens": max_tokens,
            // llama.cpp: reuse the KV cache for the unchanged prompt prefix
            // (system prompt + tool schemas), the main latency win.
            "cache_prompt": true,
        });
        // Plain text, e.g. a summary: no tools to offer.
        if tools.is_empty() {
            if let Some(b) = body.as_object_mut() {
                b.remove("tools");
                b.remove("tool_choice");
            }
        }
        let resp =
            self.http.post(format!("{}/v1/chat/completions", self.config.url)).json(&body).send().await.context("contacting the local model")?;
        let status = resp.status();
        let v: Value = resp.json().await.context("reading the model's reply")?;
        if !status.is_success() {
            bail!("model server returned {status}: {}", v["error"]["message"].as_str().unwrap_or(""));
        }
        let message = v["choices"][0]["message"].clone();
        if message.is_null() {
            return Err(anyhow!("model server returned no message"));
        }
        let content = message["content"].as_str().unwrap_or("").trim().to_owned();
        let tool_calls = message["tool_calls"]
            .as_array()
            .map(|calls| {
                calls
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        let raw = c["function"]["arguments"].as_str().unwrap_or("{}");
                        let arguments = match serde_json::from_str::<Value>(raw) {
                            Ok(Value::Object(m)) => Ok(m),
                            _ => Err(raw.to_owned()),
                        };
                        ToolCall {
                            id: c["id"].as_str().map(str::to_owned).unwrap_or_else(|| format!("call_{i}")),
                            name: c["function"]["name"].as_str().unwrap_or("").to_owned(),
                            arguments,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Reply { content, tool_calls, message })
    }
}
