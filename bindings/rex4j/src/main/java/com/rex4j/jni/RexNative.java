package com.rex4j.jni;

import com.rex4j.RexConfig;
import com.rex4j.RexData;
import com.rex4j.RexHandler;

/** JNI 本地方法接口 对应 Rust 端的 FFI 函数 */
public class RexNative {
  static {
    // 加载动态库 (librex4j.so / rex4j.dll)
    NativeLibrary.load();
  }

  /**
   * 初始化 Rex 客户端
   *
   * @param config 配置对象
   * @param handler 消息处理器
   * @return 指向 Rust Arc<dyn RexClientTrait> 的指针
   */
  public static native long init(RexConfig config, RexHandler handler);

  /**
   * 发送数据
   *
   * @param client 客户端句柄
   * @param data RexData
   */
  public static native void send(long client, RexData data);

  /**
   * 获取连接状态
   *
   * @param client 客户端句柄
   * @return 状态码 (0=Disconnected, 1=Connecting, 2=Connected, 3=Reconnecting)
   */
  public static native int getConnectionState(long client);

  /**
   * 关闭客户端连接
   *
   * @param client 客户端句柄
   */
  public static native void close(long client);

  /**
   * 注册 title（向服务器注册新的标识）
   *
   * @param client 客户端句柄
   * @param title 要注册的 title
   */
  public static native void registerTitle(long client, String title);

  /**
   * 删除 title
   *
   * @param client 客户端句柄
   * @param title 要删除的 title
   */
  public static native void deleteTitle(long client, String title);

  /**
   * 获取客户端 ID
   *
   * @param client 客户端句柄
   * @return 客户端 ID (128-bit UUID as byte[16])
   */
  public static native byte[] getClientId(long client);
}
