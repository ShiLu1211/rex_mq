package com.rex4j.example;

import com.rex4j.*;
import java.nio.ByteBuffer;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;
import org.HdrHistogram.Histogram;
import org.HdrHistogram.Recorder;
import picocli.CommandLine;
import picocli.CommandLine.Option;

public class RexEngine implements Runnable {
  private static final int TIMESTAMP_LENGTH = 8;

  @Option(
      names = {"-h", "--host"},
      required = true,
      description = "ip地址")
  String host;

  @Option(
      names = {"-p", "--port"},
      required = true,
      description = "端口")
  int port;

  @Option(
      names = {"-t", "--title"},
      defaultValue = "",
      description = "多个title用;隔开")
  String title;

  @Option(
      names = {"-y", "--type"},
      required = true,
      description = "rcv or snd")
  String type;

  @Option(
      names = {"-s", "--size"},
      defaultValue = "1024",
      description = "包大小")
  int size;

  @Option(
      names = {"-T", "--time"},
      defaultValue = "60",
      description = "发送持续时间，单位秒")
  int time;

  @Option(
      names = {"-i", "--interval"},
      defaultValue = "50",
      description = "发送间隔，单位微秒")
  long interval;

  @Option(
      names = {"-c", "--command"},
      defaultValue = "Title")
  RexCommand command;

  public static void main(String[] args) {
    System.exit(new CommandLine(new RexEngine()).execute(args));
  }

  @Override
  public void run() {
    if ("rcv".equalsIgnoreCase(type)) {
      rcv();
    } else if ("snd".equalsIgnoreCase(type)) {
      snd();
    } else {
      System.out.println("Invalid type value");
    }
  }

  void rcv() {
    AtomicLong rcv_count = new AtomicLong(0);
    Recorder recorder = new Recorder(3);

    ScheduledExecutorService statsScheduler = Executors.newSingleThreadScheduledExecutor();
    statsScheduler.scheduleAtFixedRate(
        () -> {
          Histogram hist = recorder.getIntervalHistogram();
          long count = hist.getTotalCount();
          if (count > 0) {
            double p50 = hist.getValueAtPercentile(50) / 1_000.0;
            double p90 = hist.getValueAtPercentile(90) / 1_000.0;
            double p95 = hist.getValueAtPercentile(95) / 1_000.0;
            double p98 = hist.getValueAtPercentile(98) / 1_000.0;
            double p99 = hist.getValueAtPercentile(99) / 1_000.0;
            double max = hist.getMaxValue() / 1_000.0;
            double min = hist.getMinValue() / 1_000.0;
            double mean = hist.getMean() / 1_000.0;

            System.out.printf(
                "tps: %d, mean: %.3f µs, min: %.0f µs, P50: %.0f µs, P90: %.0f µs, P95: %.0f µs,"
                    + " P98: %.0f µs, P99: %.0f µs, max: %.0f µs%n",
                count, mean, min, p50, p90, p95, p98, p99, max);
          }
        },
        1,
        1,
        TimeUnit.SECONDS);
    RexConfig config = RexConfig.builder(host, port, title).build();
    RexClient client =
        new RexClient(
            config,
            new RexHandler() {
              @Override
              public void onLogin(RexClient client, RexData data) {
                System.out.println("rcv client login ok");
              }

              @Override
              public void onMessage(RexClient client, RexData data) {
                long now = System.nanoTime();
                long timestamp = timestamp(data.getData());

                if (timestamp == -10086) {
                  System.out.printf("receive total: [%d]%n", rcv_count.get());
                  System.exit(0);
                  return;
                }
                rcv_count.incrementAndGet();
                long latency = now - timestamp;
                if (latency < 0) {
                  System.err.printf("timestamp: %d, now: %d%n", timestamp, now);
                }

                recorder.recordValue(latency);
              }
            });

    while (true) {
      try {
        Thread.sleep(1000);
      } catch (InterruptedException e) {
        throw new RuntimeException(e);
      }
    }
  }

  void snd() {
    RexConfig config = RexConfig.builder(host, port, title).build();
    RexClient client =
        new RexClient(
            config,
            new RexHandler() {
              @Override
              public void onLogin(RexClient client, RexData data) {
                System.out.println("snd client login ok");
              }

              @Override
              public void onMessage(RexClient client, RexData data) {
                System.out.println("snd receive: " + data.getCommand());
              }
            });

    long end = System.currentTimeMillis() + time * 1000L;
    long intervalNanos = interval * 1_000L;
    long start = 0;
    long snd_count = 0;

    do {
      start = System.nanoTime();

      RexData data = RexData.builder(command).title(title).data(wrapData(size)).build();

      client.send(data);
      snd_count += 1;

      while (true) {
        long elapsed = System.nanoTime() - start;
        if (elapsed > intervalNanos) {
          break;
        }
      }

    } while (System.currentTimeMillis() < end);

    ByteBuffer buffer = ByteBuffer.wrap(new byte[TIMESTAMP_LENGTH]);
    buffer.putLong(-10086);
    RexData stopData = RexData.builder(command).title(title).data(buffer.array()).build();
    client.send(stopData);

    System.out.printf("send total: [%d]%n", snd_count);

    client.close();
  }

  byte[] wrapData(int size) {
    ByteBuffer buffer = ByteBuffer.wrap(new byte[size]);
    long timestamp = System.nanoTime();
    buffer.putLong(timestamp);
    return buffer.array();
  }

  long timestamp(byte[] data) {
    ByteBuffer buffer = ByteBuffer.wrap(data, 0, TIMESTAMP_LENGTH);
    return buffer.getLong();
  }
}
