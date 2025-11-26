package com.rex4j.example;

import com.rex4j.RexClient;
import com.rex4j.RexConfig;
import com.rex4j.RexData;
import com.rex4j.RexHandler;

public class Recv {
  public static void main(String[] args) {
    RexConfig config =
        RexConfig.builder("127.0.0.1", 8881, "rcv").protocol(RexConfig.Protocol.TCP).build();

    RexClient client =
        new RexClient(
            config,
            new RexHandler() {
              @Override
              public void onLogin(RexClient client, RexData data) {
                System.out.println("recv login");
              }

              @Override
              public void onMessage(RexClient client, RexData data) {
                System.out.println("recv message: " + data.getDataStr());
              }
            });

    while (true) {
      try {
        Thread.sleep(1000);
      } catch (InterruptedException e) {
        e.printStackTrace();
      }
    }
  }
}
