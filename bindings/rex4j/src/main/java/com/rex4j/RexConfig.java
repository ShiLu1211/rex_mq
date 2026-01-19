package com.rex4j;

/** Rex 客户端配置 对应 Rust 的 RexClientConfig */
public class RexConfig {

  /** 协议类型枚举 */
  public enum Protocol {
    TCP(0),
    QUIC(1),
    WEBSOCKET(2);

    private final int value;

    Protocol(int value) {
      this.value = value;
    }

    public int getValue() {
      return value;
    }
  }

  private Protocol protocol = Protocol.TCP;
  private String host;
  private int port;
  private String title;
  private String address; // 缓存 address 计算结果

  // 对应 Rust 配置项
  private long idleTimeout = 10; // 空闲超时（秒）
  private long pongWait = 5; // 心跳响应等待时间（秒）
  private int maxReconnectAttempts = 5; // 最大重连次数
  private int maxBufferSize = 8 * 1024 * 1024; // 最大缓冲区大小（8MB）

  /**
   * 构造函数
   *
   * @param host 服务器地址
   * @param port 服务器端口
   * @param title 客户端标识（多个用分号分隔）
   */
  public RexConfig(String host, int port, String title) {
    if (host == null || host.isEmpty()) {
      throw new IllegalArgumentException("Host cannot be null or empty");
    }
    if (port <= 0 || port > 65535) {
      throw new IllegalArgumentException("Invalid port: " + port);
    }
    if (title == null || title.isEmpty()) {
      throw new IllegalArgumentException("Title cannot be null or empty");
    }

    this.host = host;
    this.port = port;
    this.title = title;
    this.address = host + ":" + port;
  }

  /** 构建器模式 */
  public static Builder builder(String host, int port, String title) {
    return new Builder(host, port, title);
  }

  public static class Builder {
    private final RexConfig config;

    private Builder(String host, int port, String title) {
      this.config = new RexConfig(host, port, title);
    }

    public Builder protocol(Protocol protocol) {
      config.protocol = protocol;
      return this;
    }

    public Builder idleTimeout(long seconds) {
      config.idleTimeout = seconds;
      return this;
    }

    public Builder pongWait(long seconds) {
      config.pongWait = seconds;
      return this;
    }

    public Builder maxReconnectAttempts(int attempts) {
      config.maxReconnectAttempts = attempts;
      return this;
    }

    public Builder maxBufferSize(int size) {
      config.maxBufferSize = size;
      return this;
    }

    public RexConfig build() {
      return config;
    }
  }

  // Getters
  public Protocol getProtocol() {
    return protocol;
  }

  public String getHost() {
    return host;
  }

  public int getPort() {
    return port;
  }

  public String getTitle() {
    return title;
  }

  public String getAddress() {
    return address;
  }

  public long getIdleTimeout() {
    return idleTimeout;
  }

  public long getPongWait() {
    return pongWait;
  }

  public int getMaxReconnectAttempts() {
    return maxReconnectAttempts;
  }

  public int getMaxBufferSize() {
    return maxBufferSize;
  }

  // Setters
  public void setProtocol(Protocol protocol) {
    this.protocol = protocol;
  }

  public void setTitle(String title) {
    if (title == null || title.isEmpty()) {
      throw new IllegalArgumentException("Title cannot be null or empty");
    }
    this.title = title;
  }

  public void setIdleTimeout(long idleTimeout) {
    this.idleTimeout = idleTimeout;
  }

  public void setPongWait(long pongWait) {
    this.pongWait = pongWait;
  }

  public void setMaxReconnectAttempts(int maxReconnectAttempts) {
    this.maxReconnectAttempts = maxReconnectAttempts;
  }

  public void setMaxBufferSize(int maxBufferSize) {
    this.maxBufferSize = maxBufferSize;
  }

  @Override
  public String toString() {
    return String.format(
        "RexConfig{protocol=%s, address=%s, title='%s', "
            + "idleTimeout=%d, pongWait=%d, maxReconnectAttempts=%d}",
        protocol, getAddress(), title, idleTimeout, pongWait, maxReconnectAttempts);
  }
}
