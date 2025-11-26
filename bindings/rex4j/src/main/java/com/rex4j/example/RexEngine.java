package com.rex4j.example;

import picocli.CommandLine;
import picocli.CommandLine.Option;

public class RexEngine implements Runnable {

  @Option(
      names = {"-h", "--host"},
      required = true)
  String host;

  @Option(
      names = {"-p", "--port"},
      required = true)
  int port;

  @Option(
      names = {"-t", "--title"},
      required = true)
  String title;

  public static void main(String[] args) {
    System.exit(new CommandLine(new RexEngine()).execute(args));
  }

  @Override
  public void run() {
    System.out.printf("Server: %s:%d Title=%s\n", host, port, title);
  }
}
