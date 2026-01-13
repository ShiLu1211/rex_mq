#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rex_core::{RexCommand, RexData};
use std::hint::black_box;

/// 创建测试消息
fn make_msg(title: &str, data_size: usize) -> RexData {
    RexData::new(RexCommand::Title, title, &vec![0x42u8; data_size])
}

/// 序列化性能
fn serialize(c: &mut Criterion) {
    let mut g = c.benchmark_group("serialize");
    for &size in &[64, 256, 1024, 4096, 16384] {
        let msg = make_msg("test_room", size);
        g.throughput(Throughput::Bytes(size as u64));
        g.bench_with_input(BenchmarkId::from_parameter(size), &msg, |b, msg| {
            b.iter(|| black_box(msg.pack_ref()));
        });
    }
}

/// 有无 title 比较
fn serialize_title_compare(c: &mut Criterion) {
    let mut g = c.benchmark_group("serialize_title");
    let with_title = make_msg("test_room_123", 1024);
    let without_title = make_msg("", 1024);
    g.bench_function("with_title", |b| {
        b.iter(|| black_box(with_title.pack_ref()))
    });
    g.bench_function("without_title", |b| {
        b.iter(|| black_box(without_title.pack_ref()))
    });
}

/// 反序列化性能
fn deserialize(c: &mut Criterion) {
    let mut g = c.benchmark_group("deserialize");
    for &size in &[64, 256, 1024, 4096, 16384] {
        let msg = make_msg("test_room", size);
        let serialized = msg.pack_ref();
        g.throughput(Throughput::Bytes(size as u64));
        g.bench_with_input(BenchmarkId::from_parameter(size), serialized, |b, data| {
            b.iter(|| {
                let mut buf = data.clone();
                black_box(RexData::try_deserialize(&mut buf).unwrap());
            });
        });
    }
}

/// 序列化+反序列化往返
fn roundtrip(c: &mut Criterion) {
    let mut g = c.benchmark_group("roundtrip");
    for &size in &[64, 256, 1024, 4096] {
        let msg = make_msg("test_room", size);
        g.throughput(Throughput::Bytes(size as u64));
        g.bench_with_input(BenchmarkId::from_parameter(size), &msg, |b, msg| {
            b.iter(|| {
                let mut buf = msg.pack_ref().clone();
                black_box(RexData::try_deserialize(&mut buf).unwrap());
            });
        });
    }
}

criterion_group!(
    benches,
    serialize,
    serialize_title_compare,
    deserialize,
    roundtrip,
);
criterion_main!(benches);
