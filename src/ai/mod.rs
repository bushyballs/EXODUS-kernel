/// Hoags AI — on-device AI subsystem for Genesis
///
/// A privacy-first AI that runs entirely on-device:
///   1. Inference engine: runs GGUF/ONNX models locally
///   2. Natural language interface: understand user intent
///   3. System assistant: help with tasks, answer questions
///   4. Smart search: semantic search across files
///   5. Code completion: AI-assisted shell and editor
///   6. NLP pipeline: tokenization, POS, NER, sentiment
///   7. RAG: retrieval-augmented generation
///   8. Vision: object detection, OCR, scene classification
///   9. Voice: wake word, ASR, TTS, speaker ID
///  10. Smart suggestions: adaptive, contextual, predictive
///  11. Automation: triggers, conditions, routines
///  12. Knowledge graph: entities, relations, inference
///  13. On-device training: fine-tune, federated, personalization
///
/// No data ever leaves the device unless explicitly requested.
///
/// Inspired by: Apple Intelligence (on-device), Ollama (local LLM),
/// llama.cpp (efficient inference). All code is original.
use crate::{serial_print, serial_println};
pub mod anomaly;
pub mod assistant;
pub mod automation;
pub mod classifier;
pub mod context;
pub mod document_analysis;
pub mod embeddings;
pub mod feedback_loop;
pub mod inference;
pub mod intelligence;
pub mod knowledge;
pub mod memory_graph;
pub mod model_router;
pub mod multimodal;
pub mod neural_bus;
pub mod nlp;
pub mod personalization;
pub mod planning;
pub mod prompt_engine;
pub mod rag;
pub mod reasoning;
pub mod recommendation;
pub mod safety_filter;
pub mod self_improve;
pub mod sentiment;
pub mod smart_suggest;
pub mod summarizer;
pub mod tool_call;
pub mod training;
pub mod vendor_matching;
pub mod vision;
pub mod voice;

pub fn init() {
    inference::init();
    embeddings::init();
    nlp::init();
    rag::init();
    vision::init();
    voice::init();
    knowledge::init();
    smart_suggest::init();
    automation::init();
    training::init();
    intelligence::init();
    self_improve::init();
    personalization::init();
    reasoning::init();
    multimodal::init();
    assistant::init();
    context::init();
    planning::init();
    tool_call::init();
    memory_graph::init();
    feedback_loop::init();
    model_router::init();
    prompt_engine::init();
    safety_filter::init();
    document_analysis::init();
    vendor_matching::init();
    sentiment::init();
    summarizer::init();
    classifier::init();
    anomaly::init();
    recommendation::init();
    neural_bus::init();
    serial_println!("  AI: inference, embeddings, NLP, RAG, vision, voice, knowledge, suggestions, automation, training, intelligence, self-improve, personalization, reasoning, multimodal, assistant, context, planning, tool_call, memory_graph, feedback_loop, model_router, prompt_engine, safety_filter, document_analysis, vendor_matching, sentiment, summarizer, classifier, anomaly, recommendation, neural_bus");
}
