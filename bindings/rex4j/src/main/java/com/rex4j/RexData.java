package com.rex4j;

import java.nio.charset.StandardCharsets;

/** Rex 数据包对象 对应 Rust 的 RexData 结构 */
public class RexData {
  private RexCommand command;
  private String title;
  private byte[] data;

  private RexData() {
    this.data = new byte[0];
    this.command = RexCommand.Unknown;
    this.title = "";
  }

  /** 构建器模式创建 RexData */
  public static Builder builder(RexCommand command) {
    return new Builder(command);
  }

  public static class Builder {
    private final RexData rexData;

    private Builder(RexCommand command) {
      this.rexData = new RexData();
      this.rexData.command = command;
    }

    public Builder title(String title) {
      rexData.title = title != null ? title : "";
      return this;
    }

    public Builder data(byte[] data) {
      rexData.data = data != null ? data : new byte[0];
      return this;
    }

    public Builder data(String data) {
      return data(data.getBytes(StandardCharsets.UTF_8));
    }

    public RexData build() {
      return rexData;
    }
  }

  // Getters
  public RexCommand getCommand() {
    return command;
  }

  public String getTitle() {
    return title;
  }

  public byte[] getData() {
    return data;
  }

  public String getDataStr() {
    return new String(data, StandardCharsets.UTF_8);
  }

  // Setters
  public void setTitle(String title) {
    this.title = title != null ? title : "";
  }

  public void setData(byte[] data) {
    this.data = data != null ? data : new byte[0];
  }

  public void setData(String data) {
    this.data = data.getBytes(StandardCharsets.UTF_8);
  }
}
