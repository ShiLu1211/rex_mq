import argparse
import asyncio
import threading
import time

from rex4p import ClientConfig, RexClient, RexCommand, Protocol


# ==========================================
# 📊 Benchmark Statistics Handler
# ==========================================
class BenchmarkHandler:
    """
    Used by the Receiver to track incoming messages and calculate stats.
    """

    def __init__(self, total_expected, mode="async"):
        self.total_expected = total_expected
        self.received_count = 0
        self.start_time = None
        self.end_time = None
        self.mode = mode

        # 获取当前的事件循环（需要在 async 上下文中初始化，或者在 start 时获取）
        # 如果是在 main 中初始化，可以直接获取
        try:
            self.loop = asyncio.get_running_loop()
        except RuntimeError:
            self.loop = None

        # Events to signal completion
        self.async_done_event = asyncio.Event()
        self.sync_done_event = threading.Event()

    def on_login(self, data):
        print(f"[{self.mode.upper()} RECEIVER] ✅ Logged in successfully.")

    def on_message(self, data):
        # Initialize start time on the very first message
        if self.received_count == 0:
            self.start_time = time.time()
            print(
                f"[{self.mode.upper()} RECEIVER] ⏱️ First message received. Timer started."
            )

        self.received_count += 1

        # Progress Log every 10%
        if self.received_count % (max(1, self.total_expected // 10)) == 0:
            print(
                f"[{self.mode.upper()} RECEIVER] 📥 Progress: {self.received_count}/{self.total_expected}"
            )

        # Check if done
        if self.received_count >= self.total_expected:
            self.end_time = time.time()
            if self.mode == "async":
                # ✅ 修复：使用 threadsafe 方法通知主线程
                if self.loop and not self.loop.is_closed():
                    self.loop.call_soon_threadsafe(self.async_done_event.set)
                else:
                    # Fallback 如果 loop 没获取到（理论上不会发生）
                    pass
            else:
                self.sync_done_event.set()

    def print_stats(self):
        if not self.start_time or not self.end_time:
            print("❌ No messages received.")
            return

        elapsed = self.end_time - self.start_time
        rate = self.received_count / elapsed if elapsed > 0 else 0

        print("\n" + "=" * 50)
        print(f"📊 {self.mode.upper()} TEST RESULTS")
        print("=" * 50)
        print(f"✅ Total Messages: {self.received_count}")
        print(f"⏱️  Elapsed Time:   {elapsed:.4f} s")
        print(f"🚀 Throughput:     {rate:.2f} msg/s")
        print(f"📉 Latency (Avg):  {(elapsed/self.received_count)*1000:.3f} ms/msg")
        print("=" * 50 + "\n")


# ==========================================
# 🔄 ASYNC TEST IMPLEMENTATION
# ==========================================
async def run_async_benchmark(host, count, batch_size):
    print(f"\n🚀 Starting ASYNC Benchmark ({count} messages)...")

    # 1. Setup Receiver
    recv_handler = BenchmarkHandler(count, mode="async")
    recv_config = ClientConfig(host, Protocol.tcp(), "bench_receiver", recv_handler)
    receiver = RexClient()
    await receiver.connect(recv_config)

    # 2. Setup Sender (No handler needed for sender usually, or a dummy one)
    send_config = ClientConfig(
        host, Protocol.tcp(), "bench_sender", MyHandler()
    )  # Reusing basic handler
    sender = RexClient()
    await sender.connect(send_config)

    # Wait for connections
    while not (await receiver.is_connected() and await sender.is_connected()):
        await asyncio.sleep(0.1)

    print("🔌 Both clients connected. Starting transmission...")

    # 3. Sending Phase
    send_start = time.time()

    # Create tasks for concurrent sending
    tasks = []
    for i in range(count):
        # Note: We set target="bench_receiver" to route message to the receiver
        task = sender.send_text(
            RexCommand.Title, f"Async Payload {i}", title="bench_receiver"
        )
        tasks.append(task)

        # Batch control (optional throttle to prevent local memory overflow on massive tests)
        if len(tasks) >= batch_size:
            await asyncio.gather(*tasks)
            tasks = []

    if tasks:
        await asyncio.gather(*tasks)

    send_elapsed = time.time() - send_start
    print(f"📤 [SENDER] Finished sending {count} messages in {send_elapsed:.2f}s")

    # 4. Wait for Receiver to finish
    print("⏳ [RECEIVER] Waiting for all messages to arrive...")
    try:
        await asyncio.wait_for(recv_handler.async_done_event.wait(), timeout=30)
        recv_handler.print_stats()
    except asyncio.TimeoutError:
        print(
            f"❌ Timed out! Only received {recv_handler.received_count}/{count} messages."
        )

    # Cleanup
    await sender.close()
    await receiver.close()


# ==========================================
# 🔄 SYNC TEST IMPLEMENTATION
# ==========================================
def run_sync_benchmark(host, count):
    print(f"\n🚀 Starting SYNC Benchmark ({count} messages)...")

    # 1. Setup Receiver
    recv_handler = BenchmarkHandler(count, mode="sync")
    recv_config = ClientConfig(host, Protocol.tcp(), "bench_receiver", recv_handler)
    receiver = RexClient()
    receiver.connect(recv_config)

    # 2. Setup Sender
    send_config = ClientConfig(host, Protocol.tcp(), "bench_sender", MyHandler())
    sender = RexClient()
    sender.connect(send_config)

    # Wait for connections (Simple sleep loop)
    while not (receiver.is_connected() and sender.is_connected()):
        time.sleep(0.1)

    print("🔌 Both clients connected. Starting transmission...")

    # 3. Sending Phase
    # Note: Sync sending is blocking, so we don't need batching logic the same way
    send_start = time.time()
    for i in range(count):
        sender.send_text(RexCommand.Title, f"Sync Payload {i}", title="bench_receiver")

    send_elapsed = time.time() - send_start
    print(f"📤 [SENDER] Finished sending {count} messages in {send_elapsed:.2f}s")

    # 4. Wait for Receiver
    print("⏳ [RECEIVER] Waiting for all messages to arrive...")
    if recv_handler.sync_done_event.wait(timeout=30):
        recv_handler.print_stats()
    else:
        print(
            f"❌ Timed out! Only received {recv_handler.received_count}/{count} messages."
        )

    sender.close()
    receiver.close()


# Helper for dummy sender handler
class MyHandler:
    def on_login(self, data):
        pass

    def on_message(self, data):
        pass


# ==========================================
# 🏁 MAIN ENTRY POINT
# ==========================================
if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Rex4p Performance Benchmark")
    parser.add_argument(
        "--mode", choices=["async", "sync"], default="sync", help="Test mode"
    )
    parser.add_argument("--host", default="127.0.0.1:8881", help="Server address")
    parser.add_argument(
        "--count", type=int, default=10000, help="Number of messages to send"
    )
    parser.add_argument(
        "--batch", type=int, default=1000, help="Async batch size for gathering tasks"
    )

    args = parser.parse_args()

    print("=" * 60)
    print("🧪 REX4P END-TO-END BENCHMARK")
    print(f"🎯 Target: {args.host}")
    print(f"📦 Messages: {args.count}")
    print("=" * 60)

    try:
        if args.mode in ["async"]:
            asyncio.run(run_async_benchmark(args.host, args.count, args.batch))

        if args.mode in ["sync"]:
            run_sync_benchmark(args.host, args.count)

    except KeyboardInterrupt:
        print("\n🛑 Test interrupted by user.")
    except Exception as e:
        print(f"\n❌ Unexpected error: {e}")
        import traceback

        traceback.print_exc()
