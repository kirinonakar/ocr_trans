use anyhow::{Context, Result};
use serde_json::Value;

#[derive(Clone, Copy)]
pub(super) enum Format {
    Chat,
    Responses,
    Gemini,
    Ollama,
}

/// Frame bytes before UTF-8 decoding: network chunks may split a Korean character or SSE event.
pub(super) struct Decoder {
    format: Format,
    pending: Vec<u8>,
    event: Vec<String>,
    text: String,
    completed: bool,
}

impl Decoder {
    pub(super) fn new(format: Format) -> Self {
        Self {
            format,
            pending: Vec::new(),
            event: Vec::new(),
            text: String::new(),
            completed: false,
        }
    }

    pub(super) fn done(&self) -> bool {
        self.completed
    }

    pub(super) fn feed(&mut self, bytes: &[u8], update: &mut impl FnMut(&str)) -> Result<()> {
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<_> = self.pending.drain(..=end).collect();
            self.line(
                std::str::from_utf8(&line)
                    .context("Invalid UTF-8 in API stream")?
                    .trim_end_matches(['\r', '\n']),
                update,
            )?;
            if self.completed {
                self.pending.clear();
                break;
            }
        }
        anyhow::ensure!(
            self.pending.len() < 2 * 1024 * 1024,
            "API stream record is too large"
        );
        Ok(())
    }

    fn line(&mut self, line: &str, update: &mut impl FnMut(&str)) -> Result<()> {
        if matches!(self.format, Format::Ollama) {
            if !line.trim().is_empty() {
                self.record(line, update)?;
            }
        } else if line.is_empty() {
            self.flush_event(update)?;
        } else if let Some(data) = line.strip_prefix("data:") {
            self.event
                .push(data.strip_prefix(' ').unwrap_or(data).to_string());
            anyhow::ensure!(
                self.event.iter().map(String::len).sum::<usize>() < 2 * 1024 * 1024,
                "API stream event is too large"
            );
        }
        Ok(())
    }

    fn flush_event(&mut self, update: &mut impl FnMut(&str)) -> Result<()> {
        if self.event.is_empty() {
            return Ok(());
        }
        let data = std::mem::take(&mut self.event).join("\n");
        self.record(&data, update)
    }

    fn record(&mut self, data: &str, update: &mut impl FnMut(&str)) -> Result<()> {
        if data.trim() == "[DONE]" {
            self.completed = true;
            return Ok(());
        }
        let json: Value = serde_json::from_str(data).context("Invalid API stream event")?;
        if !json["error"].is_null()
            || matches!(
                json["type"].as_str(),
                Some("error" | "response.failed" | "response.incomplete")
            )
        {
            let message = json["error"]["message"]
                .as_str()
                .or_else(|| json["error"].as_str())
                .or_else(|| json["response"]["error"]["message"].as_str())
                .or_else(|| json["message"].as_str())
                .unwrap_or("Generation failed or was incomplete");
            anyhow::bail!("{}", super::single_line(message));
        }
        let delta = match self.format {
            Format::Chat => {
                self.completed = json["choices"][0]["finish_reason"].as_str().is_some();
                json["choices"][0]["delta"]["content"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            }
            Format::Responses => {
                self.completed = json["type"] == "response.completed";
                if json["type"] == "response.output_text.delta" {
                    json["delta"].as_str().unwrap_or_default().to_string()
                } else {
                    String::new()
                }
            }
            Format::Gemini => {
                self.completed = json["candidates"][0]["finishReason"].as_str().is_some();
                json["candidates"][0]["content"]["parts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|part| part["thought"].as_bool() != Some(true))
                    .filter_map(|part| part["text"].as_str())
                    .collect()
            }
            Format::Ollama => {
                self.completed = json["done"].as_bool() == Some(true);
                json["message"]["content"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            }
        };
        if !delta.is_empty() {
            self.text.push_str(&delta);
            update(&self.text);
        }
        Ok(())
    }

    pub(super) fn finish(mut self, update: &mut impl FnMut(&str)) -> Result<String> {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            self.line(
                std::str::from_utf8(&pending)
                    .context("Truncated UTF-8 in API stream")?
                    .trim_end_matches('\r'),
                update,
            )?;
        }
        self.flush_event(update)?;
        anyhow::ensure!(
            self.completed,
            "Stream ended before generation completed; partial text was not copied"
        );
        anyhow::ensure!(
            !self.text.trim().is_empty(),
            "No text found in API response. Check model compatibility."
        );
        Ok(self.text.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_all_protocols_with_arbitrary_utf8_chunk_boundaries() {
        for (format, wire) in [
            (Format::Chat, "data: {\"choices\":[{\"delta\":{\"content\":\"안녕\"},\"finish_reason\":null}]}\r\n\r\ndata: [DONE]\r\n\r\n"),
            (Format::Responses, "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\ndata: \"delta\":\"안녕\"}\n\ndata: {\"type\":\"response.completed\"}\n\n"),
            (Format::Gemini, "data: {\"candidates\":[{\"content\":{\"parts\":[{\"thought\":true,\"text\":\"hidden\"},{\"text\":\"안녕\"}]},\"finishReason\":\"STOP\"}]}\n\n"),
            (Format::Ollama, "{\"message\":{\"thinking\":\"hidden\",\"content\":\"안녕\"},\"done\":false}\n{\"done\":true}"),
        ] {
            let mut decoder = Decoder::new(format);
            let mut updates = Vec::new();
            let mut callback = |text: &str| updates.push(text.to_string());
            for byte in wire.as_bytes() { decoder.feed(&[*byte], &mut callback).unwrap(); }
            assert_eq!(decoder.finish(&mut callback).unwrap(), "안녕");
            assert_eq!(updates, ["안녕"]);
        }
    }

    #[test]
    fn partial_streams_and_stream_errors_never_become_successful_results() {
        let mut decoder = Decoder::new(Format::Chat);
        decoder
            .feed(
                b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
                &mut |_| {},
            )
            .unwrap();
        assert!(decoder.finish(&mut |_| {}).is_err());
        for format in [
            Format::Chat,
            Format::Responses,
            Format::Gemini,
            Format::Ollama,
        ] {
            let mut decoder = Decoder::new(format);
            assert!(decoder
                .record(r#"{"error":{"message":"failed"}}"#, &mut |_| {})
                .is_err());
        }
    }
}
