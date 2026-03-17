use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::format;
/// System prompt construction and optimization
///
/// Part of the AIOS AI layer. Template-based prompt builder with
/// {{variable}} placeholder substitution, prompt chaining,
/// fragment management, and prompt history tracking.
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// A registered prompt template
#[derive(Clone)]
pub struct PromptTemplate {
    pub name: String,
    pub template: String,
    pub description: String,
    /// Variable names expected by this template (extracted from {{var}} placeholders)
    pub variables: Vec<String>,
    /// Default values for variables
    pub defaults: BTreeMap<String, String>,
    /// Template priority for ordering in chains
    pub priority: u32,
}

impl PromptTemplate {
    /// Create a new template, automatically extracting variable names
    fn new(name: &str, template: &str, description: &str) -> Self {
        let variables = extract_variable_names(template);
        PromptTemplate {
            name: String::from(name),
            template: String::from(template),
            description: String::from(description),
            variables,
            defaults: BTreeMap::new(),
            priority: 0,
        }
    }

    fn with_default(mut self, var: &str, value: &str) -> Self {
        self.defaults.insert(String::from(var), String::from(value));
        self
    }

    fn with_priority(mut self, p: u32) -> Self {
        self.priority = p;
        self
    }

    /// Fill this template with the given variables, falling back to defaults
    fn fill(&self, variables: &BTreeMap<String, String>) -> String {
        let mut result = self.template.clone();

        for var_name in &self.variables {
            let placeholder = format!("{{{{{}}}}}", var_name);
            let value = variables
                .get(var_name)
                .or_else(|| self.defaults.get(var_name));

            match value {
                Some(val) => {
                    result = str_replace_all(&result, &placeholder, val);
                }
                None => {
                    // Leave placeholder as-is if no value provided
                }
            }
        }

        result
    }

    /// Check if all required variables (those without defaults) are provided
    fn missing_variables(&self, provided: &BTreeMap<String, String>) -> Vec<String> {
        self.variables
            .iter()
            .filter(|v| {
                !provided.contains_key(v.as_str()) && !self.defaults.contains_key(v.as_str())
            })
            .cloned()
            .collect()
    }
}

/// A prompt chain: sequence of templates composed into a single prompt
#[derive(Clone)]
pub struct PromptChain {
    pub name: String,
    pub template_names: Vec<String>,
    pub separator: String,
}

impl PromptChain {
    fn new(name: &str, templates: &[&str], separator: &str) -> Self {
        PromptChain {
            name: String::from(name),
            template_names: templates.iter().map(|s| String::from(*s)).collect(),
            separator: String::from(separator),
        }
    }
}

/// A historical prompt entry
struct PromptHistoryEntry {
    pub template_name: String,
    pub rendered: String,
    pub timestamp: u64,
    pub token_estimate: usize,
}

/// Builds optimized system prompts from components
pub struct PromptEngine {
    pub system_prefix: String,
    pub fragments: Vec<String>,
    pub max_tokens: usize,
    /// Template registry: name -> template
    templates: BTreeMap<String, PromptTemplate>,
    /// Prompt chains
    chains: BTreeMap<String, PromptChain>,
    /// Global variables available to all templates
    global_variables: BTreeMap<String, String>,
    /// Prompt history
    history: Vec<PromptHistoryEntry>,
    /// Maximum history entries
    max_history: usize,
    /// Logical clock
    clock: u64,
}

impl PromptEngine {
    pub fn new() -> Self {
        PromptEngine {
            system_prefix: String::new(),
            fragments: Vec::new(),
            max_tokens: 4096,
            templates: BTreeMap::new(),
            chains: BTreeMap::new(),
            global_variables: BTreeMap::new(),
            history: Vec::new(),
            max_history: 256,
            clock: 0,
        }
    }

    /// Set the system prefix that appears at the start of every prompt
    pub fn set_system_prefix(&mut self, prefix: &str) {
        self.system_prefix = String::from(prefix);
    }

    /// Set the maximum token budget
    pub fn set_max_tokens(&mut self, max: usize) {
        self.max_tokens = max;
    }

    /// Register a new prompt template
    pub fn register_template(&mut self, template: PromptTemplate) {
        self.templates.insert(template.name.clone(), template);
    }

    /// Register a template from raw parts
    pub fn register(&mut self, name: &str, template_str: &str, description: &str) {
        let template = PromptTemplate::new(name, template_str, description);
        self.register_template(template);
    }

    /// Register a prompt chain
    pub fn register_chain(&mut self, chain: PromptChain) {
        self.chains.insert(chain.name.clone(), chain);
    }

    /// Set a global variable available to all templates
    pub fn set_variable(&mut self, key: &str, value: &str) {
        self.global_variables
            .insert(String::from(key), String::from(value));
    }

    /// Set multiple global variables
    pub fn set_variables(&mut self, vars: &[(&str, &str)]) {
        for &(key, value) in vars {
            self.global_variables
                .insert(String::from(key), String::from(value));
        }
    }

    /// Add a prompt fragment
    pub fn add_fragment(&mut self, fragment: &str) {
        self.fragments.push(String::from(fragment));
    }

    /// Remove a fragment by index
    pub fn remove_fragment(&mut self, index: usize) -> bool {
        if index < self.fragments.len() {
            self.fragments.remove(index);
            true
        } else {
            false
        }
    }

    /// Clear all fragments
    pub fn clear_fragments(&mut self) {
        self.fragments.clear();
    }

    /// Build the complete prompt from system prefix + fragments
    pub fn build(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        let mut total_estimate = 0usize;

        if !self.system_prefix.is_empty() {
            parts.push(&self.system_prefix);
            total_estimate += estimate_tokens(&self.system_prefix);
        }

        for fragment in &self.fragments {
            let frag_tokens = estimate_tokens(fragment);
            if total_estimate + frag_tokens > self.max_tokens {
                break; // Token budget exceeded
            }
            parts.push(fragment);
            total_estimate += frag_tokens;
        }

        parts.join("\n\n")
    }

    /// Fill a named template with variables
    pub fn fill_template(
        &mut self,
        name: &str,
        variables: &BTreeMap<String, String>,
    ) -> Option<String> {
        let template = self.templates.get(name)?.clone();

        // Merge global variables with provided variables (provided take precedence)
        let mut merged = self.global_variables.clone();
        for (k, v) in variables {
            merged.insert(k.clone(), v.clone());
        }

        let rendered = template.fill(&merged);

        // Record in history
        self.clock = self.clock.saturating_add(1);
        self.history.push(PromptHistoryEntry {
            template_name: String::from(name),
            rendered: rendered.clone(),
            timestamp: self.clock,
            token_estimate: estimate_tokens(&rendered),
        });
        if self.history.len() > self.max_history {
            self.history.remove(0);
        }

        Some(rendered)
    }

    /// Fill a template with a simple list of key-value pairs
    pub fn fill(&mut self, name: &str, vars: &[(&str, &str)]) -> Option<String> {
        let mut map = BTreeMap::new();
        for &(k, v) in vars {
            map.insert(String::from(k), String::from(v));
        }
        self.fill_template(name, &map)
    }

    /// Execute a prompt chain: fill each template in sequence and join
    pub fn execute_chain(
        &mut self,
        chain_name: &str,
        variables: &BTreeMap<String, String>,
    ) -> Option<String> {
        let chain = self.chains.get(chain_name)?.clone();
        let mut parts: Vec<String> = Vec::new();
        let mut total_tokens = 0usize;

        for template_name in &chain.template_names {
            if let Some(rendered) = self.fill_template(template_name, variables) {
                let tokens = estimate_tokens(&rendered);
                if total_tokens + tokens > self.max_tokens {
                    break;
                }
                total_tokens += tokens;
                parts.push(rendered);
            }
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(&chain.separator))
        }
    }

    /// Get the list of registered template names
    pub fn template_names(&self) -> Vec<String> {
        self.templates.keys().cloned().collect()
    }

    /// Get a template by name
    pub fn get_template(&self, name: &str) -> Option<&PromptTemplate> {
        self.templates.get(name)
    }

    /// Get variables required by a template (that have no defaults)
    pub fn required_variables(&self, name: &str) -> Vec<String> {
        match self.templates.get(name) {
            Some(t) => t.missing_variables(&self.global_variables),
            None => Vec::new(),
        }
    }

    /// Get the prompt history
    pub fn history_rendered(&self, last_n: usize) -> Vec<String> {
        let start = if self.history.len() > last_n {
            self.history.len() - last_n
        } else {
            0
        };
        self.history[start..]
            .iter()
            .map(|h| h.rendered.clone())
            .collect()
    }

    /// Get total tokens used across all history entries
    pub fn total_history_tokens(&self) -> usize {
        self.history.iter().map(|h| h.token_estimate).sum()
    }

    /// Number of registered templates
    pub fn template_count(&self) -> usize {
        self.templates.len()
    }

    /// Number of registered chains
    pub fn chain_count(&self) -> usize {
        self.chains.len()
    }

    /// Clear prompt history
    pub fn clear_history(&mut self) {
        self.history.clear();
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Extract {{variable_name}} placeholders from a template string
fn extract_variable_names(template: &str) -> Vec<String> {
    let mut names = Vec::new();
    let bytes = template.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i + 3 < len {
        if bytes[i] == b'{' && bytes[i + 1] == b'{' {
            // Found opening {{
            let start = i + 2;
            let mut end = start;
            while end + 1 < len {
                if bytes[end] == b'}' && bytes[end + 1] == b'}' {
                    let name_bytes = &bytes[start..end];
                    if let Ok(name) = core::str::from_utf8(name_bytes) {
                        let trimmed = name.trim();
                        if !trimmed.is_empty() {
                            let name_str = String::from(trimmed);
                            if !names.contains(&name_str) {
                                names.push(name_str);
                            }
                        }
                    }
                    i = end + 2;
                    break;
                }
                end += 1;
            }
            if end + 1 >= len {
                break;
            }
        } else {
            i += 1;
        }
    }

    names
}

/// Replace all occurrences of `from` with `to` in `source`
fn str_replace_all(source: &str, from: &str, to: &str) -> String {
    if from.is_empty() {
        return String::from(source);
    }

    let mut result = String::new();
    let mut remaining = source;

    while let Some(pos) = remaining.find(from) {
        result.push_str(&remaining[..pos]);
        result.push_str(to);
        remaining = &remaining[pos + from.len()..];
    }
    result.push_str(remaining);
    result
}

/// Estimate token count for a string (~4 chars per token)
fn estimate_tokens(text: &str) -> usize {
    let count = (text.len() + 3) / 4;
    if count == 0 {
        1
    } else {
        count
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static ENGINE: Mutex<Option<PromptEngine>> = Mutex::new(None);

pub fn init() {
    let mut engine = PromptEngine::new();

    engine.set_system_prefix(
        "You are the Genesis AIOS on-device assistant. You help users with system \
         tasks, answer questions, and provide intelligent suggestions. All processing \
         runs locally with no external API calls.",
    );

    // Register common templates
    engine.register_template(
        PromptTemplate::new(
            "qa",
            "Answer the following question based on the given context.\n\nContext: {{context}}\n\nQuestion: {{question}}\n\nAnswer:",
            "Question-answering template"
        )
    );

    engine.register_template(
        PromptTemplate::new(
            "summarize",
            "Summarize the following text in {{length}} sentences.\n\nText: {{text}}\n\nSummary:",
            "Text summarization template",
        )
        .with_default("length", "3"),
    );

    engine.register_template(
        PromptTemplate::new(
            "classify",
            "Classify the following text into one of these categories: {{categories}}.\n\nText: {{text}}\n\nCategory:",
            "Text classification template"
        )
    );

    engine.register_template(
        PromptTemplate::new(
            "analyze",
            "Analyze the following {{document_type}} and extract key information.\n\nDocument: {{content}}\n\nAnalysis:",
            "Document analysis template"
        ).with_default("document_type", "document")
    );

    engine.register_template(
        PromptTemplate::new(
            "chat",
            "{{system_prompt}}\n\n{{history}}\n\nUser: {{user_message}}\n\nAssistant:",
            "Chat conversation template",
        )
        .with_default("system_prompt", "You are a helpful assistant.")
        .with_default("history", ""),
    );

    engine.register_template(
        PromptTemplate::new(
            "code",
            "Write {{language}} code that {{task}}.\n\nRequirements:\n{{requirements}}\n\nCode:",
            "Code generation template",
        )
        .with_default("language", "Rust")
        .with_default("requirements", "- Clean, well-documented code"),
    );

    engine.register_template(
        PromptTemplate::new(
            "extract",
            "Extract the following information from the text:\n{{fields}}\n\nText: {{text}}\n\nExtracted data:",
            "Information extraction template"
        )
    );

    // Register chains
    engine.register_chain(PromptChain::new(
        "analyze_and_summarize",
        &["analyze", "summarize"],
        "\n\n---\n\n",
    ));

    engine.register_chain(PromptChain::new(
        "classify_and_qa",
        &["classify", "qa"],
        "\n\n",
    ));

    // Set default global variables
    engine.set_variable("os_name", "Genesis AIOS");
    engine.set_variable("version", "1.0");

    *ENGINE.lock() = Some(engine);
    crate::serial_println!("    [prompt_engine] Prompt engine ready (7 templates, 2 chains)");
}

/// Fill a named template with variables
pub fn fill(name: &str, vars: &[(&str, &str)]) -> Option<String> {
    ENGINE.lock().as_mut().and_then(|e| e.fill(name, vars))
}

/// Build the current prompt from prefix + fragments
pub fn build() -> String {
    ENGINE
        .lock()
        .as_ref()
        .map(|e| e.build())
        .unwrap_or_else(String::new)
}

/// Add a fragment to the prompt
pub fn add_fragment(fragment: &str) {
    if let Some(e) = ENGINE.lock().as_mut() {
        e.add_fragment(fragment);
    }
}
