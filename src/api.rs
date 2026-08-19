use serde::Serialize;
use anyhow::{Result, Context};
use reqwest::Client;
use base64::{Engine as _, engine::general_purpose::STANDARD};

#[derive(Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(rename = "generationConfig", skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Serialize)]
struct GeminiGenerationConfig {
    temperature: f32,
    #[serde(rename = "thinkingConfig", skip_serializing_if = "Option::is_none")]
    thinking_config: Option<GeminiThinkingConfig>,
}

#[derive(Serialize)]
struct GeminiThinkingConfig {
    #[serde(rename = "thinkingLevel", skip_serializing_if = "Option::is_none")]
    thinking_level: Option<String>,
    #[serde(rename = "thinkingBudget", skip_serializing_if = "Option::is_none")]
    thinking_budget: Option<i32>,
}

#[derive(Serialize)]
struct GeminiContent {
    parts: Vec<GeminiPart>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum GeminiPart {
    Text { text: String },
    InlineData { inline_data: InlineData },
}

#[derive(Serialize)]
struct InlineData {
    mime_type: String,
    data: String,
}

pub struct ApiClient {
    client: Client,
    endpoint: String,
    api_key: String,
    model: String,
    system_prompt: String,
    temperature: f32,
    thinking_level: String,
    provider: String,
}

impl ApiClient {
    pub fn new(
        client: Client,
        endpoint: String,
        api_key: String,
        model: String,
        system_prompt: String,
        temperature: f32,
        thinking_level: String,
        provider: String,
    ) -> Self {
        // Normalize endpoint: ensure it doesn't end with /v1 or /v1beta if it's the base
        let mut endpoint = endpoint.trim_end_matches('/').to_string();
        if endpoint.is_empty() {
            endpoint = "https://generativelanguage.googleapis.com".to_string();
        }
        Self {
            client,
            endpoint,
            api_key,
            model,
            system_prompt,
            temperature,
            thinking_level,
            provider,
        }
    }

    pub async fn translate_image(&self, img: &image::RgbaImage) -> Result<String> {
        let jpeg_data = self.prepare_image(img)?;
        let base64_image = STANDARD.encode(&jpeg_data);
        
        if self.is_gemini_endpoint() {
            self.call_gemini(base64_image).await
        } else if self.is_ollama_endpoint() {
            self.call_ollama(base64_image).await
        } else if self.is_responses_model() {
            self.call_openai_responses(base64_image).await
        } else {
            self.call_openai_compatible(base64_image).await
        }
    }

    fn is_gemini_endpoint(&self) -> bool {
        self.endpoint.contains("googleapis.com")
    }

    fn is_ollama_endpoint(&self) -> bool {
        let endpoint = self.endpoint.to_lowercase();
        endpoint.contains("ollama.com")
            || endpoint.contains(":11434")
            || endpoint.ends_with("/api")
    }

    fn normalized_thinking_level(&self) -> Option<String> {
        let level = self.thinking_level.trim().to_lowercase();
        match level.as_str() {
            "disable" | "disabled" => Some("disable".to_string()),
            "low" | "medium" | "high" | "xhigh" | "max" => Some(level),
            _ => None,
        }
    }

    fn gemini_thinking_config(&self) -> Option<GeminiThinkingConfig> {
        let level = self.normalized_thinking_level()?;
        if level == "disable" {
            return Some(GeminiThinkingConfig {
                thinking_level: None,
                thinking_budget: Some(0),
            });
        }

        let model = self.model.to_lowercase();
        if model.contains("2.5") {
            let budget = match level.as_str() {
                "low" => 1024,
                "medium" => 4096,
                "high" | "xhigh" | "max" => 8192,
                _ => return None,
            };
            Some(GeminiThinkingConfig {
                thinking_level: None,
                thinking_budget: Some(budget),
            })
        } else {
            let thinking_level = match level.as_str() {
                "low" => "LOW",
                "medium" => "MEDIUM",
                "high" | "xhigh" | "max" => "HIGH",
                _ => return None,
            };
            Some(GeminiThinkingConfig {
                thinking_level: Some(thinking_level.to_string()),
                thinking_budget: None,
            })
        }
    }

    fn is_unsloth_endpoint(&self) -> bool {
        self.provider == "Unsloth Desktop"
    }

    fn is_responses_model(&self) -> bool {
        self.provider == "OpenCode Go" && self.model == "gpt-5.6-luna"
    }

    fn is_opencode_mimo_model(&self) -> bool {
        self.provider == "OpenCode Go" && self.model.starts_with("mimo-v2.5")
    }

    fn is_opencode_provider(&self) -> bool {
        self.provider == "OpenCode Go" || self.provider == "OpenCode Zen"
    }

    fn apply_openai_thinking(&self, payload: &mut serde_json::Value) {
        let Some(level) = self.normalized_thinking_level() else {
            return;
        };

        if self.is_unsloth_endpoint() {
            payload["enable_thinking"] = serde_json::Value::Bool(level != "disable");
            if level != "disable" {
                payload["reasoning_effort"] = serde_json::Value::String(level);
            }
            return;
        }

        if self.is_opencode_mimo_model() {
            if level == "disable" {
                payload["thinking"] = serde_json::json!({ "type": "disabled" });
            } else {
                let effort = match level.as_str() {
                    "xhigh" | "max" => "high",
                    _ => level.as_str(),
                };
                payload["reasoning_effort"] = serde_json::Value::String(effort.to_string());
            }
            return;
        }

        if level == "disable" {
            payload["reasoning_effort"] = serde_json::Value::String("none".to_string());
        } else {
            payload["reasoning_effort"] = serde_json::Value::String(level);
        }
    }

    fn apply_ollama_thinking(&self, payload: &mut serde_json::Value) {
        let Some(level) = self.normalized_thinking_level() else {
            return;
        };
        payload["think"] = if level == "disable" {
            serde_json::Value::Bool(false)
        } else {
            let level = if level == "xhigh" { "max" } else { level.as_str() };
            serde_json::Value::String(level.to_string())
        };
    }

    fn apply_responses_thinking(&self, payload: &mut serde_json::Value) {
        let Some(level) = self.normalized_thinking_level() else {
            return;
        };

        let effort = if level == "disable" {
            "none".to_string()
        } else {
            level
        };
        payload["reasoning"] = serde_json::json!({ "effort": effort });
    }

    fn ollama_api_url(&self, path: &str) -> String {
        let base = self.endpoint.trim_end_matches('/');
        let path = path.trim_start_matches('/');
        if base.ends_with("/api") {
            format!("{}/{}", base, path)
        } else {
            format!("{}/api/{}", base, path)
        }
    }

    fn prepare_image(&self, img: &image::RgbaImage) -> Result<Vec<u8>> {
        let (w, h) = img.dimensions();
        let new_w = 1024;
        let new_h = if w > 0 {
            (h as f32 * (new_w as f32 / w as f32)) as u32
        } else {
            h
        };

        log::info!("Resizing image from {}x{} to {}x{}", w, h, new_w, new_h);
        let resized = image::imageops::resize(img, new_w, new_h.max(1), image::imageops::FilterType::Lanczos3);
        let rgb_img = image::DynamicImage::ImageRgba8(resized).to_rgb8();

        let mut buffer = Vec::new();
        rgb_img.write_to(&mut std::io::Cursor::new(&mut buffer), image::ImageFormat::Jpeg)
            .context("Failed to encode image to JPEG")?;
        
        log::info!("Image prepared, size: {} bytes", buffer.len());
        Ok(buffer)
    }

    async fn call_gemini(&self, base64_image: String) -> Result<String> {

        // Normalize model name: it shouldn't contain spaces and ideally starts with models/ if not provided, 
        // though the URL below adds it.
        let model = self.model.trim().to_lowercase().replace(" ", "-");
        // Remove 'models/' if it was already included in the string so we don't double it
        let model = model.strip_prefix("models/").unwrap_or(&model);

        let url = format!(
            "{}/v1beta/models/{}:generateContent",
            self.endpoint, model
        );

        let thinking_config = self.gemini_thinking_config();

        let request = GeminiRequest {
            contents: vec![GeminiContent {
                parts: vec![
                    GeminiPart::Text {
                        text: self.system_prompt.clone(),
                    },
                    GeminiPart::InlineData {
                        inline_data: InlineData {
                            mime_type: "image/jpeg".to_string(),
                            data: base64_image,
                        },
                    },
                ],
            }],
            generation_config: Some(GeminiGenerationConfig {
                temperature: self.temperature,
                thinking_config,
            }),
        };

        log::info!("Sending Gemini request to URL: {} (Model: {})", url, model);
        let mut req = self.client.post(&url).json(&request);
        let api_key = self.api_key.trim();
        if !api_key.is_empty() {
            req = req.header("x-goog-api-key", api_key);
        }

        let response = req.send().await.context("HTTP request failed")?;
        
        let status = response.status();
        log::info!("Gemini Response Status: {}", status);

        if !status.is_success() {
            let err_body = response.text().await?;
            log::error!("Gemini API Error ({}): {}", status, err_body);
            anyhow::bail!("Gemini API Error ({}): {}", status, err_body);
        }

        let json_text = response.text().await.context("Failed to get response text")?;
        let json: serde_json::Value = serde_json::from_str(&json_text).context("Failed to parse JSON")?;
        
        let mut full_text = String::new();
        if let Some(parts) = json["candidates"][0]["content"]["parts"].as_array() {
            for part in parts {
                if let Some(t) = part["text"].as_str() {
                    full_text.push_str(t);
                }
            }
        }

        if full_text.is_empty() {
             anyhow::bail!("No text found in Gemini response parts. Check for safety filters or model compatibility.");
        }
        
        log::info!("Total Gemini response text received (length: {})", full_text.len());
        let mut processed_text = full_text;
        
        // Gemma models sometimes output lines starting with * (notes, thoughts, etc.)
        // We filter these out if the model name contains "gemma"
        if self.model.to_lowercase().contains("gemma") {
            processed_text = processed_text.lines()
                .filter(|line| !line.trim_start().starts_with('*'))
                .collect::<Vec<_>>()
                .join("\n");
        }
        
        Ok(processed_text.trim().to_string())
    }

    async fn call_openai_compatible(&self, base64_image: String) -> Result<String> {

        let url = if self.endpoint.ends_with("/chat/completions") {
            self.endpoint.clone()
        } else {
            format!("{}/chat/completions", self.endpoint)
        };
        
        let mut payload = serde_json::json!({
            "model": self.model,
            "messages": [
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": self.system_prompt.clone() },
                        { "type": "image_url", "image_url": { "url": format!("data:image/jpeg;base64,{}", base64_image) } }
                    ]
                }
            ]
        });
        // MiMo V2.5 does not support custom temperature values. Omitting the
        // field avoids a 400 from the OpenCode Go MiMo adapter.
        if !self.is_opencode_provider() {
            payload["temperature"] = serde_json::json!(self.temperature);
        }
        self.apply_openai_thinking(&mut payload);

        let mut req = self.client.post(&url).json(&payload);
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }

        log::info!("Sending AI request to: {}", url);
        let response = req.send().await.context("HTTP request failed")?;
        
        if !response.status().is_success() {
            let status = response.status();
            let err_body = response.text().await?;
            log::error!("API Request Error ({}): {}", status, err_body);
            anyhow::bail!("OpenAI API Error ({}): {}", status, err_body);
        }

        let json: serde_json::Value = response.json().await.context("Failed to decode JSON")?;
        log::info!("AI Response received.");
        
        let text = json["choices"][0]["message"]["content"]
            .as_str()
            .context("Missing text in OpenAI response. Check if your model supports Vision!")?;
            
        Ok(text.trim().to_string())
    }

    async fn call_openai_responses(&self, base64_image: String) -> Result<String> {
        let url = if self.endpoint.ends_with("/responses") {
            self.endpoint.clone()
        } else {
            format!("{}/responses", self.endpoint)
        };

        // OpenCode Go's GPT 5.6 Luna is exposed through the Responses API.
        // Responses uses input_text/input_image rather than chat-completion
        // messages and image_url content parts.
        let mut payload = serde_json::json!({
            "model": self.model,
            "input": [
                {
                    "role": "user",
                    "content": [
                        { "type": "input_text", "text": self.system_prompt.clone() },
                        { "type": "input_image", "image_url": format!("data:image/jpeg;base64,{}", base64_image) }
                    ]
                }
            ]
        });
        self.apply_responses_thinking(&mut payload);

        let mut req = self.client.post(&url).json(&payload);
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }

        log::info!("Sending OpenAI Responses request to: {}", url);
        let response = req.send().await.context("HTTP request failed")?;

        if !response.status().is_success() {
            let status = response.status();
            let err_body = response.text().await?;
            log::error!("OpenAI Responses API Error ({}): {}", status, err_body);
            anyhow::bail!("OpenAI Responses API Error ({}): {}", status, err_body);
        }

        let json: serde_json::Value = response
            .json()
            .await
            .context("Failed to decode OpenAI Responses JSON")?;

        if let Some(text) = json["output_text"].as_str() {
            return Ok(text.trim().to_string());
        }

        let mut text = String::new();
        if let Some(output) = json["output"].as_array() {
            for item in output {
                if let Some(content) = item["content"].as_array() {
                    for part in content {
                        if part["type"].as_str() == Some("output_text") {
                            if let Some(value) = part["text"].as_str() {
                                text.push_str(value);
                            }
                        }
                    }
                }
            }
        }

        if text.trim().is_empty() {
            anyhow::bail!("Missing text in OpenAI Responses response. Check model compatibility.");
        }

        Ok(text.trim().to_string())
    }

    async fn call_ollama(&self, base64_image: String) -> Result<String> {
        let url = self.ollama_api_url("chat");

        let mut payload = serde_json::json!({
            "model": self.model,
            "messages": [
                {
                    "role": "user",
                    "content": self.system_prompt.clone(),
                    "images": [base64_image]
                }
            ],
            "stream": false,
            "options": {
                "temperature": self.temperature
            }
        });
        self.apply_ollama_thinking(&mut payload);

        let mut req = self.client.post(&url).json(&payload);
        let api_key = self.api_key.trim();
        if !api_key.is_empty() {
            req = req.bearer_auth(api_key);
        }

        log::info!("Sending Ollama request to: {}", url);
        let response = req.send().await.context("HTTP request failed")?;

        if !response.status().is_success() {
            let status = response.status();
            let err_body = response.text().await?;
            log::error!("Ollama API Error ({}): {}", status, err_body);
            anyhow::bail!("Ollama API Error ({}): {}", status, err_body);
        }

        let json: serde_json::Value = response.json().await.context("Failed to decode JSON")?;
        log::info!("Ollama response received.");

        let text = json["message"]["content"]
            .as_str()
            .context("Missing text in Ollama response. Check if your model supports Vision!")?;

        Ok(text.trim().to_string())
    }

    pub async fn get_models(&self) -> Result<Vec<String>> {
        if self.is_gemini_endpoint() {
            return self.get_gemini_models().await;
        }
        if self.is_ollama_endpoint() {
            return self.get_ollama_models().await;
        }

        let url = if self.endpoint.ends_with("/models") {
            self.endpoint.clone()
        } else if self.endpoint.ends_with("/v1") {
            format!("{}/models", self.endpoint)
        } else {
            format!("{}/v1/models", self.endpoint)
        };

        let mut req = self.client.get(&url);
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }

        let response = req.send().await.context("Failed to fetch models")?;
        if !response.status().is_success() {
            anyhow::bail!("Failed to fetch models: {}", response.status());
        }

        let json: serde_json::Value = response.json().await?;
        let mut models = Vec::new();

        if let Some(data) = json["data"].as_array() {
            for m in data {
                if let Some(id) = m["id"].as_str() {
                    models.push(id.trim().to_string());
                }
            }
        }

        Ok(models)
    }

    async fn get_ollama_models(&self) -> Result<Vec<String>> {
        let url = self.ollama_api_url("tags");

        let mut req = self.client.get(&url);
        let api_key = self.api_key.trim();
        if !api_key.is_empty() {
            req = req.bearer_auth(api_key);
        }

        let response = req.send().await.context("Failed to fetch Ollama models")?;
        if !response.status().is_success() {
            anyhow::bail!("Failed to fetch Ollama models: {}", response.status());
        }

        let json: serde_json::Value = response.json().await?;
        let mut models = Vec::new();

        if let Some(data) = json["models"].as_array() {
            for m in data {
                if let Some(name) = m["name"].as_str().or_else(|| m["model"].as_str()) {
                    models.push(name.trim().to_string());
                }
            }
        }

        Ok(models)
    }

    async fn get_gemini_models(&self) -> Result<Vec<String>> {
        let url = if self.endpoint.ends_with("/models") {
            self.endpoint.clone()
        } else {
            format!("{}/v1beta/models", self.endpoint)
        };

        let mut req = self.client.get(&url);
        let api_key = self.api_key.trim();
        if !api_key.is_empty() {
            req = req.header("x-goog-api-key", api_key);
        }

        let response = req.send().await.context("Failed to fetch Gemini models")?;
        if !response.status().is_success() {
            anyhow::bail!("Failed to fetch Gemini models: {}", response.status());
        }

        let json: serde_json::Value = response.json().await?;
        let mut models = Vec::new();

        if let Some(data) = json["models"].as_array() {
            for m in data {
                if let Some(name) = m["name"].as_str() {
                    let name = name.trim();
                    models.push(name.strip_prefix("models/").unwrap_or(name).to_string());
                }
            }
        }

        Ok(models)
    }
}
