pub mod anomaly;
pub mod data_pipeline;
pub mod model;
pub mod neural_net;
pub mod ops;
pub mod optimizer;
pub mod policy;
pub mod tensor;

use crate::serial_println;

pub fn init() {
    neural_net::init();
    optimizer::init();
    data_pipeline::init();
    serial_println!(
        "  [ml] EXODUS model loaded: layers={} weights={}B",
        model::ANOMALY_MODEL.layer_count,
        model::ANOMALY_MODEL.total_weight_bytes
    );
}
