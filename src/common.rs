use std::time::{SystemTime, UNIX_EPOCH};

use uuid::Uuid;

pub fn new_uuid() -> usize {
    Uuid::new_v4().as_u128() as usize
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
