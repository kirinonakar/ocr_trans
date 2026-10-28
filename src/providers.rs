//! Provider metadata shared by settings, UI initialization and request routing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Protocol {
    ChatCompletions,
    Gemini,
    Ollama,
    Responses,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Thinking {
    ReasoningEffort,
    GeminiLevel,
    GeminiBudget,
    Ollama,
    Unsloth,
    Mimo,
}

pub(crate) struct ModelOverride {
    pub(crate) model: &'static str,
    pub(crate) prefix: bool,
    pub(crate) protocol: Protocol,
    pub(crate) thinking: Thinking,
}

pub(crate) struct Provider {
    pub(crate) name: &'static str,
    pub(crate) model_key: &'static str,
    pub(crate) endpoint: &'static str,
    pub(crate) models: &'static [&'static str],
    pub(crate) protocol: Protocol,
    pub(crate) thinking: Thinking,
    pub(crate) overrides: &'static [ModelOverride],
    pub(crate) key_target: Option<&'static str>,
    pub(crate) key_file: Option<&'static str>,
    pub(crate) key_env: Option<&'static str>,
    pub(crate) default_key: &'static str,
    pub(crate) auto_sync: bool,
    pub(crate) session_header: bool,
}

pub(crate) const PROVIDER_LMSTUDIO: &str = "LMStudio";

// Order is also the order in the provider dropdown. The first model is the default.
pub(crate) static PROVIDERS: &[Provider] = &[
    Provider {
        name: PROVIDER_LMSTUDIO,
        model_key: "lm_model",
        endpoint: "http://localhost:1234/v1",
        models: &[
            "unsloth/gemma-4-26b-a4b-it",
            "qwen/qwen3.5-9b",
            "translate-gemma-12b-it",
            "gemma-4-e4b-it",
            "gemma-4-31b-it",
            "qwen3.5-4b",
        ],
        protocol: Protocol::ChatCompletions,
        thinking: Thinking::ReasoningEffort,
        overrides: &[],
        key_target: None,
        key_file: None,
        key_env: None,
        default_key: "lm-studio",
        auto_sync: true,
        session_header: false,
    },
    Provider {
        name: "Google Gemini",
        model_key: "gemini_model",
        endpoint: "https://generativelanguage.googleapis.com",
        models: &[
            "gemini-flash-lite-latest",
            "gemini-flash-latest",
            "gemini-pro-latest",
            "gemma-4-26b-a4b-it",
            "gemma-4-31b-it",
        ],
        protocol: Protocol::Gemini,
        thinking: Thinking::GeminiLevel,
        overrides: &[ModelOverride {
            model: "gemini-2.5",
            prefix: true,
            protocol: Protocol::Gemini,
            thinking: Thinking::GeminiBudget,
        }],
        key_target: Some("ocr_trans/google-gemini-api-key"),
        key_file: Some("gemini.txt"),
        key_env: None,
        default_key: "",
        auto_sync: false,
        session_header: false,
    },
    Provider {
        name: "Cerebras",
        model_key: "cerebras_model",
        endpoint: "https://api.cerebras.ai/v1",
        models: &["gemma-4-31b", "gpt-oss-120b", "zai-glm-4.7"],
        protocol: Protocol::ChatCompletions,
        thinking: Thinking::ReasoningEffort,
        overrides: &[],
        key_target: Some("ocr_trans/cerebras-api-key"),
        key_file: Some("cerebras.txt"),
        key_env: None,
        default_key: "",
        auto_sync: false,
        session_header: false,
    },
    Provider {
        name: "Ollama",
        model_key: "ollama_model",
        endpoint: "http://localhost:11434/api",
        models: &[
            "gemma4",
            "llava",
            "moondream",
            "qwen2.5vl:7b",
            "gpt-oss:120b-cloud",
        ],
        protocol: Protocol::Ollama,
        thinking: Thinking::Ollama,
        overrides: &[],
        key_target: None,
        key_file: None,
        key_env: None,
        default_key: "",
        auto_sync: true,
        session_header: false,
    },
    Provider {
        name: "Ollama Cloud",
        model_key: "ollama_cloud_model",
        endpoint: "https://ollama.com/api",
        models: &["gemma4:31b", "gpt-oss:120b", "gpt-oss:20b"],
        protocol: Protocol::Ollama,
        thinking: Thinking::Ollama,
        overrides: &[],
        key_target: Some("ocr_trans/ollama-cloud-api-key"),
        key_file: Some("ollama_cloud.txt"),
        key_env: Some("OLLAMA_API_KEY"),
        default_key: "",
        auto_sync: true,
        session_header: false,
    },
    Provider {
        name: "Unsloth Desktop",
        model_key: "unsloth_model",
        endpoint: "http://localhost:8888/v1",
        models: &["default"],
        protocol: Protocol::ChatCompletions,
        thinking: Thinking::Unsloth,
        overrides: &[],
        key_target: Some("ocr_trans/unsloth-api-key"),
        key_file: None,
        key_env: Some("UNSLOTH_STUDIO_AUTH_TOKEN"),
        default_key: "",
        auto_sync: true,
        session_header: false,
    },
    Provider {
        name: "OpenCode Go",
        model_key: "opencode_go_model",
        endpoint: "https://opencode.ai/zen/go/v1",
        models: &[
            "kimi-k3",
            "glm-5.3",
            "deepseek-v4-flash",
            "mimo-v2.5",
            "gpt-5.6-luna",
        ],
        protocol: Protocol::ChatCompletions,
        thinking: Thinking::ReasoningEffort,
        overrides: &[
            ModelOverride {
                model: "gpt-5.6-luna",
                prefix: false,
                protocol: Protocol::Responses,
                thinking: Thinking::ReasoningEffort,
            },
            ModelOverride {
                model: "mimo-v2.5",
                prefix: true,
                protocol: Protocol::ChatCompletions,
                thinking: Thinking::Mimo,
            },
        ],
        key_target: Some("ocr_trans/opencode-go-api-key"),
        key_file: None,
        key_env: Some("OPENCODE_GO_API_KEY"),
        default_key: "",
        auto_sync: true,
        session_header: true,
    },
    Provider {
        name: "OpenCode Zen",
        model_key: "opencode_zen_model",
        endpoint: "https://opencode.ai/zen/v1",
        models: &[
            "gpt-5.5",
            "claude-sonnet-4-6",
            "deepseek-v4-flash",
            "kimi-k3",
        ],
        protocol: Protocol::ChatCompletions,
        thinking: Thinking::ReasoningEffort,
        overrides: &[],
        key_target: Some("ocr_trans/opencode-zen-api-key"),
        key_file: None,
        key_env: Some("OPENCODE_ZEN_API_KEY"),
        default_key: "",
        auto_sync: true,
        session_header: true,
    },
];

pub(crate) fn provider(name: &str) -> &'static Provider {
    PROVIDERS
        .iter()
        .find(|item| item.name == name)
        .unwrap_or(&PROVIDERS[0])
}

impl Provider {
    fn model_override(&self, model: &str) -> Option<&ModelOverride> {
        self.overrides.iter().find(|rule| {
            if rule.prefix {
                model.starts_with(rule.model)
            } else {
                model == rule.model
            }
        })
    }

    pub(crate) fn protocol(&self, model: &str) -> Protocol {
        self.model_override(model)
            .map_or(self.protocol, |rule| rule.protocol)
    }

    pub(crate) fn thinking(&self, model: &str) -> Thinking {
        self.model_override(model)
            .map_or(self.thinking, |rule| rule.thinking)
    }

    pub(crate) fn model_options(&self, saved: &str, legacy_model: &str) -> (Vec<String>, usize) {
        let mut models: Vec<String> = self.models.iter().map(|model| (*model).into()).collect();
        if self.name == PROVIDER_LMSTUDIO
            && !legacy_model.is_empty()
            && !models.iter().any(|m| m == legacy_model)
        {
            models.insert(0, legacy_model.into());
        }
        if !saved.is_empty() && !models.iter().any(|model| model == saved) {
            models.push(saved.into());
        }
        let index = models.iter().position(|model| model == saved).unwrap_or(0);
        (models, index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_have_unique_names_keys_and_usable_defaults() {
        let mut names = std::collections::HashSet::new();
        let mut keys = std::collections::HashSet::new();
        let mut targets = std::collections::HashSet::new();
        for definition in PROVIDERS {
            assert!(names.insert(definition.name));
            assert!(keys.insert(definition.model_key));
            assert!(!definition.models.is_empty());
            assert!(reqwest::Url::parse(definition.endpoint).is_ok());
            if let Some(target) = definition.key_target {
                assert!(targets.insert(target));
            }
            let (models, index) =
                definition.model_options("saved-custom-model", "legacy-local-model");
            assert_eq!(models[index], "saved-custom-model");
            let (models, index) = definition.model_options("", "");
            assert_eq!(models[index], definition.models[0]);
        }
        assert_eq!(provider("unknown provider").name, PROVIDER_LMSTUDIO);
    }

    #[test]
    fn model_overrides_are_scoped_to_the_provider() {
        let go = provider("OpenCode Go");
        assert_eq!(go.protocol("gpt-5.6-luna"), Protocol::Responses);
        assert_eq!(go.protocol("gpt-5.6-luna-other"), Protocol::ChatCompletions);
        assert_eq!(go.thinking("mimo-v2.5-variant"), Thinking::Mimo);
        assert_eq!(
            provider(PROVIDER_LMSTUDIO).protocol("gpt-5.6-luna"),
            Protocol::ChatCompletions
        );
        assert_eq!(
            provider("OpenCode Zen").thinking("mimo-v2.5"),
            Thinking::ReasoningEffort
        );
        assert_eq!(
            provider("Google Gemini").thinking("gemini-2.5-flash"),
            Thinking::GeminiBudget
        );
    }
}
