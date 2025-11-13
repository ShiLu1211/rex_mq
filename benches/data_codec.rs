use bytes::BytesMut;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rex_mq::protocol::{RexCommand, RexData};
use std::hint::black_box;

/// 创建测试消息
fn make_msg(title: Option<&str>, data_size: usize) -> RexData {
    let mut builder = RexData::builder(RexCommand::Title)
        .source(12345678901234567890u128)
        .target(98765432109876543210u128);

    if let Some(t) = title {
        builder = builder.title(t);
    }

    builder
        .data(BytesMut::from(vec![0x42u8; data_size].as_slice()))
        .build()
}

/// 创建批量消息
fn make_batch(count: usize, data_size: usize) -> Vec<RexData> {
    (0..count)
        .map(|i| {
            RexData::builder(RexCommand::Title)
                .source(i as u128)
                .target((i + 1) as u128)
                .title(format!("room_{}", i % 100))
                .data(BytesMut::from(vec![0x42u8; data_size].as_slice()))
                .build()
        })
        .collect()
}

/// 序列化性能
fn serialize(c: &mut Criterion) {
    let mut g = c.benchmark_group("serialize");
    for &size in &[64, 256, 1024, 4096, 16384] {
        let msg = make_msg(Some("test_room"), size);
        g.throughput(Throughput::Bytes(size as u64));
        g.bench_with_input(BenchmarkId::from_parameter(size), &msg, |b, msg| {
            b.iter(|| black_box(msg.serialize()));
        });
    }
}

/// 有无 title 比较
fn serialize_title_compare(c: &mut Criterion) {
    let mut g = c.benchmark_group("serialize_title");
    let with_title = make_msg(Some("test_room_123"), 1024);
    let without_title = make_msg(None, 1024);
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
        let msg = make_msg(Some("test_room"), size);
        let serialized = msg.serialize();
        g.throughput(Throughput::Bytes(size as u64));
        g.bench_with_input(BenchmarkId::from_parameter(size), &serialized, |b, data| {
            b.iter(|| {
                let mut buf = data.clone();
                black_box(RexData::deserialize(&mut buf).unwrap());
            });
        });
    }
}

/// 序列化+反序列化往返
fn roundtrip(c: &mut Criterion) {
    let mut g = c.benchmark_group("roundtrip");
    for &size in &[64, 256, 1024, 4096] {
        let msg = make_msg(Some("test_room"), size);
        g.throughput(Throughput::Bytes(size as u64));
        g.bench_with_input(BenchmarkId::from_parameter(size), &msg, |b, msg| {
            b.iter(|| {
                let mut buf = msg.serialize();
                black_box(RexData::deserialize(&mut buf).unwrap());
            });
        });
    }
}

/// 流式解析性能
fn try_deserialize(c: &mut Criterion) {
    let mut g = c.benchmark_group("try_deserialize");
    let msg = make_msg(Some("test_room"), 1024);
    let serialized = msg.serialize();
    g.bench_function("try_deserialize", |b| {
        b.iter(|| {
            let mut buf = serialized.clone();
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
                    let mut buf = msg.serialize();
                    black_box(RexData::deserialize(&mut buf).unwrap());
                }
            });
        });
    }
}

/// CRC32 计算性能
fn crc32(c: &mut Criterion) {
    let mut g = c.benchmark_group("crc32");
    for &size in &[64, 256, 1024, 4096, 16384] {
        let data = vec![0x42u8; size];
        g.throughput(Throughput::Bytes(size as u64));
        g.bench_with_input(BenchmarkId::from_parameter(size), &data, |b, d| {
            b.iter(|| {
                use crc32fast::Hasher;
                let mut h = Hasher::new();
                h.update(d);
                black_box(h.finalize());
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
    crc32
);
criterion_main!(benches);
