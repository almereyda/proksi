use std::sync::Arc;

use dashmap::DashMap;

pub struct Certificate {
    pub key: Vec<u8>,
    pub certificate: Vec<u8>,
}

pub type CertificateStore = Arc<DashMap<String, Certificate>>;
