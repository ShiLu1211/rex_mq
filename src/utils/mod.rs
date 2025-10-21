pub mod common;
mod time;

pub use time::*;

use uuid::Uuid;

pub fn new_uuid() -> u128 {
    Uuid::new_v4().as_u128()
}

pub fn force_set_value<T>(p: *const T, v: T) {
    unsafe {
        std::ptr::write(p as *mut T, v);
    }
}
