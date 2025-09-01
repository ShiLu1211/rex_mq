use uuid::Uuid;

pub fn new_uuid() -> usize {
    Uuid::new_v4().as_u128() as usize
}
