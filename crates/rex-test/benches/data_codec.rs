#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]
use bytes::{BufMut, BytesMut};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rex_core::{RexCommand, RexData};
use std::hint::black_box;

/// 创建测试消息
fn make_msg(title: &str, data_size: usize) -> RexData {
    RexData::new(
        RexCommand::Title,
        12345678901234567890u128,
        title.to_string(),
        vec![0x42u8; data_size],
    )
}

/// 创建批量消息
fn make_batch(count: usize, data_size: usize) -> Vec<RexData> {
    (0..count)
        .map(|i| {
            RexData::new(
                RexCommand::Title,
                i as u128,
                format!("room_{}", i % 100),
                vec![0x42u8; data_size],
            )
        })
        .collect()
}

/// 序列化性能
fn serialize(c: &mut Criterion) {
    let mut g = c.benchmark_group("serialize");
    for &size in &[64, 256, 1024, 4096, 16384] {
        let msg = make_msg("test_room", size);
        g.throughput(Throughput::Bytes(size as u64));
        g.bench_with_input(BenchmarkId::from_parameter(size), &msg, |b, msg| {
            b.iter(|| black_box(msg.serialize()));
        });
    }
}

/// 有无 title 比较
fn serialize_title_compare(c: &mut Criterion) {
    let mut g = c.benchmark_group("serialize_title");
    let with_title = make_msg("test_room_123", 1024);
    let without_title = make_msg("", 1024);
    g.bench_function("with_title", |b| {
        b.iter(|| black_box(with_title.serialize()))
    });
    g.bench_function("without_title", |b| {
        b.iter(|| black_box(without_title.serialize()))
    });
}

/// 反序列化性能
fn deserialize(c: &mut Criterion) {
    let mut g = c.benchmark_group("deserialize");
    for &size in &[64, 256, 1024, 4096, 16384] {
        let msg = make_msg("test_room", size);
        let serialized = msg.serialize();
        g.throughput(Throughput::Bytes(size as u64));
        g.bench_with_input(BenchmarkId::from_parameter(size), &serialized, |b, data| {
            b.iter(|| {
                let buf = data.clone();
                black_box(RexData::decode(&buf).unwrap());
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
                let buf = msg.serialize();
                black_box(RexData::decode(&buf).unwrap());
            });
        });
    }
}

/// 流式解析性能
fn try_deserialize(c: &mut Criterion) {
    let mut g = c.benchmark_group("try_deserialize");
    let msg = make_msg("test_room", 1024);
    let serialized = msg.serialize();
    let mut buf = BytesMut::with_capacity(4 + serialized.len());
    buf.put_u32(serialized.len() as u32);
    buf.put_slice(&serialized);
    g.bench_function("try_deserialize", |b| {
        b.iter(|| {
            black_box(RexData::try_deserialize(&mut buf).unwrap());
        });
    });
}

/// 批量处理性能
fn batch(c: &mut Criterion) {
    let mut g = c.benchmark_group("batch");
    for &count in &[10, 100, 1000] {
        let msgs = make_batch(count, 512);
        g.throughput(Throughput::Elements(count as u64));
        g.bench_with_input(BenchmarkId::from_parameter(count), &msgs, |b, msgs| {
            b.iter(|| {
                for msg in msgs {
                    let buf = msg.serialize();
                    black_box(RexData::decode(&buf).unwrap());
                }
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
    try_deserialize,
    batch,
);
criterion_main!(benches);
