package com.rex4j;

/** 对应 Rust 的 RexClientHandlerTrait 用于接收来自 Rust 层的回调 */
public interface RexHandler {
  /** 对应 Rust: async fn login_ok(&self, client: Arc<RexClientInner>, data: RexData) */
  void onLogin(RexClient client, RexData data);

  /** 对应 Rust: async fn handle(&self, client: Arc<RexClientInner>, data: RexData) */
  void onMessage(RexClient client, RexData data);
}
