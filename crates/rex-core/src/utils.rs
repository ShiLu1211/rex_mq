use std::time::{Duration, SystemTime, UNIX_EPOCH};

use uuid::Uuid;

pub fn new_uuid() -> u128 {
    Uuid::new_v4().as_u128()
}

pub fn force_set_value<T>(p: *const T, v: T) {
    unsafe {
        std::ptr::write(p as *mut T, v);
    }
}

/// 获取当前秒数，如果时间早于 UNIX_EPOCH，则返回 0
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

/// 获取当前微秒，如果时间早于 UNIX_EPOCH，则返回 0
pub fn now_micros() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_micros()
}

/// 获取当前纳秒，如果时间早于 UNIX_EPOCH，则返回 0
pub fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos()
}
