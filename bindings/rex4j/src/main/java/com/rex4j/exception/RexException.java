package com.rex4j.exception;

/** Rex 客户端异常类 */
public class RexException extends RuntimeException {

  public RexException(String message) {
    super(message);
  }

  public RexException(String message, Throwable cause) {
    super(message, cause);
  }

  public RexException(Throwable cause) {
    super(cause);
  }
}
