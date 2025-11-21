use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

/// 添加时间戳
pub fn timestamp_data(mut data: Vec<u8>) -> Vec<u8> {
    if data.len() < 16 {
        data.resize(16, 0);
    }

    let time_bytes = now_micros().to_le_bytes();

    if let Some(slice) = data.get_mut(0..16) {
        slice.copy_from_slice(&time_bytes);
    }

    data
}

/// 尝试从数据中读取时间戳
pub fn timestamp(data: &[u8]) -> Option<u128> {
    data.get(0..16)
        .and_then(|slice| slice.try_into().ok())
        .map(u128::from_le_bytes)
}
