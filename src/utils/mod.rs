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

pub fn now_micros() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros()
}

pub fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

pub fn timestamp_data(data: Vec<u8>) -> Vec<u8> {
    let mut data = if data.len() < 16 {
        let mut data = data;
        data.resize(16, 0);
        data
    } else {
        data
    };
    data[0..16].copy_from_slice(now_micros().to_le_bytes().as_slice());
    data
}

pub fn timestamp(data: &[u8]) -> u128 {
    u128::from_le_bytes(data[0..16].try_into().unwrap())
}

pub fn force_set_value<T>(p: *const T, v: T) {
    unsafe {
        std::ptr::write(p as *mut T, v);
    }
}
