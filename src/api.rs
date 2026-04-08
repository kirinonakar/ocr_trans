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
    system_prompt: String,
}

impl ApiClient {
    pub fn new(endpoint: String, api_key: String, model: String, system_prompt: String) -> Self {
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
            system_prompt,
        }
    }

    pub async fn translate_image(&self, img: &image::RgbaImage) -> Result<String> {
        let jpeg_data = self.prepare_image(img)?;
        let base64_image = STANDARD.encode(&jpeg_data);
        
        if self.endpoint.contains("googleapis.com") {
            self.call_gemini(base64_image).await
        } else {
            self.call_openai_compatible(base64_image).await
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
            "{}/v1beta/models/{}:generateContent?key={}",
            self.endpoint, model, self.api_key
        );

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
