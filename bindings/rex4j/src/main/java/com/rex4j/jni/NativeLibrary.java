package com.rex4j.jni;

import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.StandardCopyOption;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

public class NativeLibrary {
  private static final Logger log = LoggerFactory.getLogger(NativeLibrary.class);

  private static final String TMP_DIR = System.getProperty("java.io.tmpdir");
  private static final String OS_NAME = System.getProperty("os.name").toLowerCase();

  private static final boolean IS_WIN = OS_NAME.contains("win");
  private static final boolean IS_MAC = OS_NAME.contains("mac");

  private static final String EXT = IS_WIN ? ".dll" : IS_MAC ? ".dylib" : ".so";
  private static final String SHORT_NAME = "rex4j";
  private static final String LIB_PREFIX = IS_WIN ? SHORT_NAME : "lib" + SHORT_NAME;
  private static final String LIB_RESOURCE = LIB_PREFIX + EXT;

  private static volatile boolean loaded = false;

  public static void load() {
    if (loaded) {
      return;
    }
    synchronized (NativeLibrary.class) {
      if (loaded) {
        return;
      }

      // === 环境变量路径 ===
      try {
        System.loadLibrary(SHORT_NAME);
        loaded = true;
        return;
      } catch (UnsatisfiedLinkError ignore) {
        log.warn("环境路径未找到 {}", LIB_RESOURCE);
      }

      // === JAR 同目录 ===
      try {
        String jarDir = getJarDirectory();
        if (!jarDir.isEmpty()) {
          Path localLib = Paths.get(jarDir, LIB_RESOURCE);
          if (Files.exists(localLib)) {
            log.info("从 JAR 同目录加载 {}", localLib);
            System.load(localLib.toAbsolutePath().toString());
            loaded = true;
            return;
          }
        }
      } catch (Exception e) {
        log.warn("加载 JAR 同目录下的 {} 失败", LIB_RESOURCE, e);
      }

      // === JAR 内部资源 ===
      try {
        log.info("尝试加载 JAR 内部资源 {}", LIB_RESOURCE);
        String libPath = extractJarLib();
        if (!libPath.isEmpty()) {
          System.load(libPath);
          loaded = true;
        }
      } catch (Exception e) {
        log.error("从 JAR 内部加载 {} 失败", LIB_RESOURCE, e);
      }
    }
  }

  private static String getJarDirectory() {
    try {
      Path jarPath =
          Paths.get(
              NativeLibrary.class.getProtectionDomain().getCodeSource().getLocation().toURI());
      Path parent = jarPath.getParent();
      return parent == null ? "" : parent.toAbsolutePath().toString();
    } catch (Exception e) {
      log.warn("获取 JAR 所在目录失败", e);
      return "";
    }
  }

  private static String extractJarLib() {
    try (InputStream is =
        Thread.currentThread().getContextClassLoader().getResourceAsStream(LIB_RESOURCE)) {
      if (is == null) {
        log.warn("{} 不存在于 classpath", LIB_RESOURCE);
        return "";
      }

      Path tmpDir = Paths.get(TMP_DIR);
      Path tmpLib = Files.createTempFile(tmpDir, LIB_PREFIX, EXT);
      tmpLib.toFile().deleteOnExit();

      Files.copy(is, tmpLib, StandardCopyOption.REPLACE_EXISTING);
      return tmpLib.toAbsolutePath().toString();

    } catch (Exception e) {
      log.warn("释放 JAR 内部 {} 失败: {}", LIB_RESOURCE, e.getMessage());
      return "";
    }
  }
}
