#!/usr/bin/env python3
import argparse
import gc
import struct
import sys
import time
from threading import Lock, Thread

from rex4p import ClientConfig, RexClient, RexCommand, RexData, Protocol


class LatencyStats:
    """延迟统计 - GC优化版本"""

    __slots__ = ("latencies", "lock")

    def __init__(self):
        self.latencies = []
        self.lock = Lock()

    def record(self, latency_ns):
        with self.lock:
            self.latencies.append(latency_ns)

    def get_and_clear(self):
        with self.lock:
            data = self.latencies
            self.latencies = []
            return data

    def calculate_stats(self, latencies):
        if not latencies:
            return None

        count = len(latencies)
        latencies.sort()
        sorted_lat = latencies  # 重命名避免复制

        def percentile(p):
            k = (count - 1) * p / 100
            f = int(k)
            c = f + 1 if f + 1 < count else f
            return sorted_lat[f] + (k - f) * (sorted_lat[c] - sorted_lat[f])

        # 只在最后计算时转换一次
        inv_count = 1.0 / count
        min_lat = sorted_lat[0]
        max_lat = sorted_lat[-1]
        total = sum(sorted_lat)

        return {
            "count": count,
            "min": min_lat / 1000,
            "max": max_lat / 1000,
            "mean": (total * inv_count) / 1000,
            "p50": percentile(50) / 1000,
            "p90": percentile(90) / 1000,
            "p95": percentile(95) / 1000,
            "p98": percentile(98) / 1000,
            "p99": percentile(99) / 1000,
        }


class RcvHandler:
    """接收端处理器 - GC优化版本"""

    __slots__ = (
        "rcv_count",
        "stats",
        "running",
        "client",
        "should_exit",
        "_struct_unpack",
        "_stop_timestamp",
        "_zero_timestamp",
    )

    def __init__(self, client=None):
        self.rcv_count = 0
        self.stats = LatencyStats()
        self.running = True
        self.client = client
        self.should_exit = False
        # 预计算常量
        self._struct_unpack = struct.Struct("<q").unpack
        self._stop_timestamp = -10086
        self._zero_timestamp = 0
        self.start_stats_thread()

    def start_stats_thread(self):
        """启动统计线程"""

        def stats_loop():
            while self.running:
                time.sleep(1)
                latencies = self.stats.get_and_clear()
                stats = self.stats.calculate_stats(latencies)

                if stats:
                    print(
                        f"tps: {stats['count']}, "
                        f"mean: {stats['mean']:.3f} µs, "
                        f"min: {stats['min']:.0f} µs, "
                        f"P50: {stats['p50']:.0f} µs, "
                        f"P90: {stats['p90']:.0f} µs, "
                        f"P95: {stats['p95']:.0f} µs, "
                        f"P98: {stats['p98']:.0f} µs, "
                        f"P99: {stats['p99']:.0f} µs, "
                        f"max: {stats['max']:.0f} µs"
                    )

        thread = Thread(target=stats_loop, daemon=True)
        thread.start()

    def on_login(self, data):
        print("rcv client login ok")

    def on_message(self, data):
        now = time.time_ns()

        # 解析时间戳 - 使用memoryview避免复制
        data_bytes = data.data
        if len(data_bytes) >= 8:
            timestamp = self._struct_unpack(data_bytes[:8])[0]

            # 检查是否是停止信号
            if timestamp == self._stop_timestamp:
                time.sleep(0.5)
                print(f"\nreceive total: [{self.rcv_count}]")
                self.running = False
                self.should_exit = True
                return

            self.rcv_count += 1
            latency = now - timestamp

            if latency < 0:
                print(
                    f"Warning: negative latency - timestamp: {timestamp}, now: {now}",
                    file=sys.stderr,
                )
            else:
                self.stats.record(latency)


class SndHandler:
    """发送端处理器"""

    __slots__ = ()

    def on_login(self, data):
        print("snd client login ok")

    def on_message(self, data):
        print(f"snd receive: {data.command}")


# 全局缓冲区，用于减少GC
_buf_struct = struct.Struct("<q")
_buf_pack_into = _buf_struct.pack_into


def wrap_data(size, timestamp, out_buf):
    """打包数据，前8字节为时间戳 - 复用缓冲区版本"""
    _buf_pack_into(out_buf, 0, timestamp)
    return bytes(out_buf[:size])


def rcv_mode(args):
    """接收模式 - GC优化版本"""
    gc.disable()  # 禁用自动GC，由程序控制
    handler = RcvHandler()
    config = ClientConfig(
        f"{args.host}:{args.port}", Protocol.tcp(), args.title, handler
    )

    client = RexClient()
    handler.client = client  # 设置client引用
    client.connect(config)

    # 等待连接
    while not client.is_connected():
        time.sleep(0.1)

    print(f"Connected to {args.host}:{args.port}, title: {args.title}")

    # 保持运行
    try:
        while not handler.should_exit:
            time.sleep(0.1)
        print("Received stop signal, exiting...")
        client.close()
        time.sleep(0.5)  # 等待关闭完成
    except KeyboardInterrupt:
        print("\nShutting down...")
        client.close()


def snd_mode(args):
    """发送模式 - GC优化版本"""
    gc.disable()  # 禁用自动GC，由程序控制
    handler = SndHandler()
    config = ClientConfig(f"{args.host}:{args.port}", Protocol.tcp(), "", handler)

    client = RexClient()
    client.connect(config)

    # 等待连接
    while not client.is_connected():
        time.sleep(0.1)

    print(f"Connected to {args.host}:{args.port}, title: {args.title}")
    print(f"Starting to send {args.size} bytes packets for {args.time} seconds...")
    print(f"Interval: {args.interval} µs")

    # 转换命令
    command_map = {
        "Title": RexCommand.Title,
        "Group": RexCommand.Group,
        "Cast": RexCommand.Cast,
    }
    command = command_map.get(args.command, RexCommand.Title)

    end_time = time.time() + args.time
    interval_ns = args.interval * 1000  # 转换为纳秒
    snd_count = 0

    # 预分配缓冲区
    buf_size = max(args.size, 8)
    out_buf = bytearray(buf_size)
    _stop_ts = -10086

    try:
        while time.time() < end_time:
            start = time.time_ns()

            # 发送数据 - 复用缓冲区
            timestamp = time.time_ns()
            data_bytes = wrap_data(args.size, timestamp, out_buf)
            data = RexData(command, args.title, data_bytes)
            client.send(data)
            snd_count += 1

            # 精确控制发送间隔
            while True:
                elapsed = time.time_ns() - start
                if elapsed >= interval_ns:
                    break

        # 发送停止信号
        print("\nSending stop signal...")
        struct.pack_into("<q", out_buf, 0, _stop_ts)
        stop_msg = RexData(command, args.title, bytes(out_buf[:8]))
        client.send(stop_msg)

        print(f"send total: [{snd_count}]")

        time.sleep(1)  # 等待停止信号发送
        client.close()

    except KeyboardInterrupt:
        print(f"\nInterrupted. send total: [{snd_count}]")
        client.close()


def main():
    parser = argparse.ArgumentParser(description="Rex Engine Performance Test")
    parser.add_argument("-H", "--host", required=True, help="服务器IP地址")
    parser.add_argument("-p", "--port", type=int, required=True, help="服务器端口")
    parser.add_argument("-t", "--title", default="", help="标题，多个用;分隔")
    parser.add_argument(
        "-y",
        "--type",
        required=True,
        choices=["rcv", "snd"],
        help="模式：rcv(接收) 或 snd(发送)",
    )
    parser.add_argument("-s", "--size", type=int, default=1024, help="包大小(字节)")
    parser.add_argument("-T", "--time", type=int, default=60, help="发送持续时间(秒)")
    parser.add_argument("-i", "--interval", type=int, default=50, help="发送间隔(微秒)")
    parser.add_argument(
        "-c",
        "--command",
        default="Title",
        choices=["Title", "Group", "Cast"],
        help="命令类型",
    )

    args = parser.parse_args()

    if args.type == "rcv":
        rcv_mode(args)
    else:
        snd_mode(args)


if __name__ == "__main__":
    main()
