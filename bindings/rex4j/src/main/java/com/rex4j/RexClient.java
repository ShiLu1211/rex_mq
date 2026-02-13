package com.rex4j;

import com.rex4j.exception.RexException;
import com.rex4j.jni.RexNative;
import java.nio.ByteBuffer;
import java.util.UUID;
import java.util.concurrent.atomic.AtomicBoolean;

/** Rex 客户端主类 对应 Rust 的 RexClientTrait 实现 */
public class RexClient implements AutoCloseable {
  // 指向 Rust Arc<dyn RexClientTrait> 的指针
  private volatile long client;
  private final RexConfig config;

  @SuppressWarnings("unused")
  private final RexHandler handler;

  private final AtomicBoolean closed = new AtomicBoolean(false);

  /**
   * 创建并连接 Rex 客户端
   *
   * @param config 客户端配置
   * @param handler 消息处理器
   */
  public RexClient(RexConfig config, RexHandler handler) {
    this.config = config;
    this.handler = handler;

    // 调用 JNI 初始化，Rust 端会创建 RexClientConfig 并调用 open_client
    this.client = RexNative.init(config, handler);

    if (this.client == 0) {
      throw new RexException("Failed to initialize RexClient native object");
    }
  }

  /** 发送数据 对应 Rust: async fn send_data(&self, data: &mut RexData) -> Result<()> */
  public void send(RexData data) {
    ensureOpen();
    RexNative.send(this.client, data);
  }

  /** 发送到指定 title */
  public void send(RexCommand command, String title, byte[] message) {
    RexData data = RexData.builder(command).title(title).data(message).build();
    send(data);
  }

  /** 获取连接状态 对应 Rust: async fn get_connection_state(&self) -> ConnectionState */
  public ConnectionState getConnectionState() {
    if (this.client == 0) {
      return ConnectionState.DISCONNECTED;
    }
    int state = RexNative.getConnectionState(this.client);
    return ConnectionState.fromOrdinal(state);
  }

  /** 检查是否已连接 */
  public boolean isConnected() {
    return getConnectionState() == ConnectionState.CONNECTED;
  }

  /** 等待连接建立 */
  public boolean waitForConnection(long timeoutMillis) throws InterruptedException {
    long start = System.currentTimeMillis();
    while (System.currentTimeMillis() - start < timeoutMillis) {
      if (isConnected()) {
        return true;
      }
      Thread.sleep(100);
    }
    return false;
  }

  /** 注册新的 title 对应 Rust 的 RegTitle 命令 */
  public void registerTitle(String title) {
    ensureOpen();
    RexNative.registerTitle(this.client, title);
  }

  /** 删除 title 对应 Rust 的 DelTitle 命令 */
  public void deleteTitle(String title) {
    ensureOpen();
    RexNative.deleteTitle(this.client, title);
  }

  /** 获取客户端唯一 ID */
  public UUID getClientId() {
    ensureOpen();
    byte[] idBytes = RexNative.getClientId(this.client);
    // 将 byte[16] 转换为 UUID
    if (idBytes == null || idBytes.length != 16) {
      throw new RexException("Invalid client ID");
    }
    ByteBuffer bb = ByteBuffer.wrap(idBytes);
    return new UUID(bb.getLong(), bb.getLong());
  }

  /** 获取配置 */
  public RexConfig getConfig() {
    return config;
  }

  /** 检查客户端是否已关闭 */
  public boolean isClosed() {
    return closed.get();
  }

  private void ensureOpen() {
    if (closed.get() || this.client == 0) {
      throw new IllegalStateException("Client is closed");
    }
  }

  /** 关闭客户端连接 对应 Rust: async fn close(&self) */
  @Override
  public synchronized void close() {
    if (closed.compareAndSet(false, true)) {
      if (this.client != 0) {
        RexNative.close(this.client);
        this.client = 0;
      }
    }
  }

  /** 连接状态枚举 映射 Rust 的 ConnectionState */
  public enum ConnectionState {
    DISCONNECTED(0), // 断开连接
    CONNECTING(1), // 连接中
    CONNECTED(2), // 已连接
    RECONNECTING(3); // 重连中

    private final int ordinal;

    ConnectionState(int ordinal) {
      this.ordinal = ordinal;
    }

    public int getOrdinal() {
      return ordinal;
    }

    public static ConnectionState fromOrdinal(int ordinal) {
      for (ConnectionState state : values()) {
        if (state.ordinal == ordinal) {
          return state;
        }
      }
      return DISCONNECTED;
    }
  }

  @Override
  public String toString() {
    return String.format(
        "RexClient{state=%s, config=%s, closed=%s}", getConnectionState(), config, closed.get());
  }
}
