use super::inference::{Model, LayerType};
use crate::sync::Mutex;
use alloc::string::String;

/// Pre-built model types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinModel {
    KeywordDetector,
    SpeechToText,
    TextToSpeech,
    ImageClassifier,
    TextEmbedding,
    FaceDetector,
    ObjectDetector,
    LanguageDetector,
}

/// Keyword detection result
pub struct KeywordResult {
    pub keyword: String,
    pub confidence: f32,
}

/// Speech recognition result
pub struct SpeechResult {
    pub text: String,
    pub confidence: f32,
    pub is_final: bool,
}

/// Image classification result
pub struct ClassifyResult {
    pub label: String,
    pub confidence: f32,
}

/// Create a keyword detection model
pub fn create_keyword_model() -> Model {
    let mut model = Model::new("keyword-detector");
    model.input_shape = alloc::vec![1, 40, 98]; // mel spectrogram
    model.output_shape = alloc::vec![1, 12]; // 12 keywords

    // Simple CNN architecture
    model.add_layer("conv1", LayerType::Conv2d {
        in_channels: 1, out_channels: 32, kernel: 3, stride: 1,
    });
    model.add_layer("relu1", LayerType::ReLU);
    model.add_layer("pool1", LayerType::MaxPool2d { kernel: 2 });
    model.add_layer("conv2", LayerType::Conv2d {
        in_channels: 32, out_channels: 64, kernel: 3, stride: 1,
    });
    model.add_layer("relu2", LayerType::ReLU);
    model.add_layer("pool2", LayerType::MaxPool2d { kernel: 2 });
    model.add_layer("flatten", LayerType::Flatten);
    model.add_layer("fc1", LayerType::Linear { in_features: 2560, out_features: 128 });
    model.add_layer("relu3", LayerType::ReLU);
    model.add_layer("fc2", LayerType::Linear { in_features: 128, out_features: 12 });
    model.add_layer("softmax", LayerType::Softmax);

    model
}

/// Create a simple text classifier
pub fn create_text_classifier(num_classes: usize) -> Model {
    let mut model = Model::new("text-classifier");
    model.input_shape = alloc::vec![1, 512]; // token embeddings
    model.output_shape = alloc::vec![1, num_classes];

    model.add_layer("embed", LayerType::Embedding { num_embeddings: 30000, dim: 128 });
    model.add_layer("norm1", LayerType::LayerNorm { features: 128 });
    model.add_layer("fc1", LayerType::Linear { in_features: 128, out_features: 64 });
    model.add_layer("relu1", LayerType::ReLU);
    model.add_layer("fc2", LayerType::Linear { in_features: 64, out_features: num_classes });
    model.add_layer("softmax", LayerType::Softmax);

    model
}

/// Create a face detection model
pub fn create_face_detector() -> Model {
    let mut model = Model::new("face-detector");
    model.input_shape = alloc::vec![1, 3, 224, 224];
    model.output_shape = alloc::vec![1, 5]; // x, y, w, h, confidence

    model.add_layer("conv1", LayerType::Conv2d {
        in_channels: 3, out_channels: 16, kernel: 3, stride: 2,
    });
    model.add_layer("relu1", LayerType::ReLU);
    model.add_layer("conv2", LayerType::Conv2d {
        in_channels: 16, out_channels: 32, kernel: 3, stride: 2,
    });
    model.add_layer("relu2", LayerType::ReLU);
    model.add_layer("pool", LayerType::MaxPool2d { kernel: 2 });
    model.add_layer("flatten", LayerType::Flatten);
    model.add_layer("fc1", LayerType::Linear { in_features: 6272, out_features: 128 });
    model.add_layer("relu3", LayerType::ReLU);
    model.add_layer("fc2", LayerType::Linear { in_features: 128, out_features: 5 });
    model.add_layer("sigmoid", LayerType::Sigmoid);

    model
}

/// Model IDs for registered built-in models
pub struct BuiltinModels {
    pub keyword_detector: Option<usize>,
    pub face_detector: Option<usize>,
    pub text_classifier: Option<usize>,
}

static BUILTINS: Mutex<BuiltinModels> = Mutex::new(BuiltinModels {
    keyword_detector: None,
    face_detector: None,
    text_classifier: None,
});

pub fn init() {
    // Register built-in models
    let kw = super::inference::register_model(create_keyword_model());
    let face = super::inference::register_model(create_face_detector());
    let text = super::inference::register_model(create_text_classifier(10));

    let mut builtins = BUILTINS.lock();
    builtins.keyword_detector = Some(kw);
    builtins.face_detector = Some(face);
    builtins.text_classifier = Some(text);

    crate::serial_println!("  [models] Built-in models registered (keyword, face, text)");
}

/// Get keyword detector model ID
pub fn keyword_detector_id() -> Option<usize> {
    BUILTINS.lock().keyword_detector
}
