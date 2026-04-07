use serde::Serialize;
use anyhow::{Result, Context};
use reqwest::Client;
use base64::{Engine as _, engine::general_purpose::STANDARD};

#[derive(Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
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
}

impl ApiClient {
    pub fn new(endpoint: String, api_key: String, model: String) -> Self {
        // Normalize endpoint: ensure it doesn't end with /v1 or /v1beta if it's the base
        let mut endpoint = endpoint.trim_end_matches('/').to_string();
        if endpoint.is_empty() {
            endpoint = "https://generativelanguage.googleapis.com".to_string();
        }
        Self {
            client: Client::new(),
            endpoint,
            api_key,
            model,
        }
    }

    pub async fn translate_image(&self, img: &image::RgbaImage) -> Result<String> {
        if self.endpoint.contains("googleapis.com") {
            self.call_gemini(img).await
        } else {
            self.call_openai_compatible(img).await
        }
    }

    async fn call_gemini(&self, img: &image::RgbaImage) -> Result<String> {
        let mut buffer = Vec::<u8>::new();
        img.write_to(&mut std::io::Cursor::new(&mut buffer), image::ImageFormat::Png)
            .context("Failed to encode image to PNG")?;
        let base64_image = STANDARD.encode(&buffer);

        let url = format!(
            "{}/v1beta/models/{}:generateContent?key={}",
            self.endpoint, self.model, self.api_key
        );

        let request = GeminiRequest {
            contents: vec![GeminiContent {
                parts: vec![
                    GeminiPart::Text {
                        text: "Extract any text from this image and translate it to Korean. If it's already Korean, return it as is. Output ONLY the translated text, no extra commentary.".to_string(),
                    },
                    GeminiPart::InlineData {
                        inline_data: InlineData {
                            mime_type: "image/png".to_string(),
                            data: base64_image,
                        },
                    },
                ],
            }],
        };

        let response = self.client.post(url)
            .json(&request)
            .send()
            .await
            .context("HTTP request failed")?;
        
        if !response.status().is_success() {
            let status = response.status();
            let err_body = response.text().await?;
            anyhow::bail!("Gemini API Error ({}): {}", status, err_body);
        }

        let json: serde_json::Value = response.json().await.context("Failed to parse JSON")?;
        
        let text = json["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .context("Missing text in Gemini response")?;
        
        Ok(text.trim().to_string())
    }

    async fn call_openai_compatible(&self, img: &image::RgbaImage) -> Result<String> {
        let mut buffer = Vec::<u8>::new();
        img.write_to(&mut std::io::Cursor::new(&mut buffer), image::ImageFormat::Png)
            .context("Failed to encode image to PNG")?;
        let base64_image = STANDARD.encode(&buffer);

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
                        { "type": "text", "text": "Extract all text from this image and translate it cleanly to Korean. Output ONLY the Korean text." },
                        { "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{}", base64_image) } }
                    ]
                }
            ]
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
}
