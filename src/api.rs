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
    #[serde(rename = "thinkingLevel")]
    thinking_level: String,
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
}

impl ApiClient {
    pub fn new(client: Client, endpoint: String, api_key: String, model: String, system_prompt: String, temperature: f32) -> Self {
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
        }
    }

    pub async fn translate_image(&self, img: &image::RgbaImage) -> Result<String> {
        let jpeg_data = self.prepare_image(img)?;
        let base64_image = STANDARD.encode(&jpeg_data);
        
        if self.is_gemini_endpoint() {
            self.call_gemini(base64_image).await
        } else {
            self.call_openai_compatible(base64_image).await
        }
    }

    fn is_gemini_endpoint(&self) -> bool {
        self.endpoint.contains("googleapis.com")
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

        let mut thinking_config = None;
        if self.model.to_lowercase().contains("gemma") {
            thinking_config = Some(GeminiThinkingConfig {
                thinking_level: "MINIMAL".to_string(),
            });
        }

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
        
        let payload = serde_json::json!({
            "model": self.model,
            "messages": [
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": self.system_prompt.clone() },
                        { "type": "image_url", "image_url": { "url": format!("data:image/jpeg;base64,{}", base64_image) } }
                    ]
                }
            ],
            "temperature": self.temperature
        });

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

    pub async fn get_models(&self) -> Result<Vec<String>> {
        if self.is_gemini_endpoint() {
            return self.get_gemini_models().await;
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
