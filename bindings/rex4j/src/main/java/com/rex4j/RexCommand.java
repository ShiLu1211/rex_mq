package com.rex4j;

import java.util.Arrays;

/** Rex 命令枚举 */
public enum RexCommand {
  Title(9901),
  TitleReturn(9902),
  Group(9903),
  GroupReturn(9904),
  Cast(9905),
  CastReturn(9906),
  Login(9907),
  LoginReturn(9908),
  Check(9909),
  CheckReturn(9910),
  RegTitle(9911),
  RegTitleReturn(9912),
  DelTitle(9913),
  DelTitleReturn(9914),

  // 用于处理未知命令防止报错
  Unknown(0);

  private final int value;

  RexCommand(int value) {
    this.value = value;
  }

  public int getValue() {
    return value;
  }

  /** 通过 int 值查找枚举，找不到返回 Unknown */
  public static RexCommand fromValue(int value) {
    return Arrays.stream(values()).filter(c -> c.value == value).findFirst().orElse(Unknown);
  }
}
