use std::sync::OnceLock;

use anyhow::Result;
use jni::{
    JNIEnv,
    objects::{GlobalRef, JFieldID, JMethodID, JStaticMethodID},
};

pub static CACHE: OnceLock<RexGlobalCache> = OnceLock::new();

pub struct RexGlobalCache {
    pub data: RexDataCache,
    pub client: RexClientCache,
    pub handler: RexHandlerCache,
    pub config: RexConfigCache,
    pub command: RexCommandCache,
}

impl RexGlobalCache {
    pub fn init(env: &mut JNIEnv) -> Result<()> {
        if CACHE.get().is_some() {
            return Ok(());
        }

        let client = RexClientCache::init(env)?;
        let handler = RexHandlerCache::init(env)?;
        let data = RexDataCache::init(env)?;
        let config = RexConfigCache::init(env)?;
        let command = RexCommandCache::init(env)?;

        // set 返回 Result，如果已设置则返回 Err，这里我们忽略 Err（表示已初始化）
        let _ = CACHE.set(RexGlobalCache {
            data,
            client,
            handler,
            config,
            command,
        });
        Ok(())
    }

    pub fn get() -> Option<&'static RexGlobalCache> {
        CACHE.get()
    }
}

pub struct RexCommandCache {
    pub cls: GlobalRef,
    pub from_value: JStaticMethodID,
    pub value: JFieldID,
}

impl RexCommandCache {
    fn init(env: &mut JNIEnv) -> Result<Self> {
        let cls_local = env.find_class("com/rex4j/RexCommand")?;
        let cls = env.new_global_ref(&cls_local)?;

        Ok(Self {
            cls,
            from_value: env.get_static_method_id(
                &cls_local,
                "fromValue",
                "(I)Lcom/rex4j/RexCommand;",
            )?,
            value: env.get_field_id(&cls_local, "value", "I")?,
        })
    }
}

pub struct RexDataCache {
    pub cls: GlobalRef,
    pub command: JFieldID,
    pub title: JFieldID,
    pub data: JFieldID,
}

impl RexDataCache {
    fn init(env: &mut JNIEnv) -> Result<Self> {
        let cls_local = env.find_class("com/rex4j/RexData")?;
        let cls = env.new_global_ref(&cls_local)?;
        Ok(Self {
            cls,
            command: env.get_field_id(&cls_local, "command", "Lcom/rex4j/RexCommand;")?,
            title: env.get_field_id(&cls_local, "title", "Ljava/lang/String;")?,
            data: env.get_field_id(&cls_local, "data", "[B")?,
        })
    }
}

pub struct RexClientCache {
    pub cls: GlobalRef,
    pub client: JFieldID,
    pub handler: JFieldID,
    pub config: JFieldID,
}

impl RexClientCache {
    fn init(env: &mut JNIEnv) -> Result<Self> {
        let cls_local = env.find_class("com/rex4j/RexClient")?;
        let cls = env.new_global_ref(&cls_local)?;
        Ok(Self {
            cls,
            client: env.get_field_id(&cls_local, "client", "J")?,
            handler: env.get_field_id(&cls_local, "handler", "Lcom/rex4j/RexHandler;")?,
            config: env.get_field_id(&cls_local, "config", "Lcom/rex4j/RexConfig;")?,
        })
    }
}

pub struct RexHandlerCache {
    pub on_login: JMethodID,
    pub on_message: JMethodID,
}

impl RexHandlerCache {
    fn init(env: &mut JNIEnv) -> Result<Self> {
        let cls = env.find_class("com/rex4j/RexHandler")?;

        Ok(Self {
            on_login: env.get_method_id(
                &cls,
                "onLogin",
                "(Lcom/rex4j/RexClient;Lcom/rex4j/RexData;)V",
            )?,
            on_message: env.get_method_id(
                &cls,
                "onMessage",
                "(Lcom/rex4j/RexClient;Lcom/rex4j/RexData;)V",
            )?,
        })
    }
}

pub struct RexConfigCache {
    pub protocol: JFieldID,
    pub host: JFieldID,
    pub port: JFieldID,
    pub title: JFieldID,
    pub idle_timeout: JFieldID,
    pub pong_wait: JFieldID,
    pub max_reconnect_attempts: JFieldID,
    pub max_buffer_size: JFieldID,
    pub protocol_value: JFieldID,
}

impl RexConfigCache {
    pub fn init(env: &mut JNIEnv) -> Result<Self> {
        let cls = env.find_class("com/rex4j/RexConfig")?;
        let protocol_cls = env.find_class("com/rex4j/RexConfig$Protocol")?;

        Ok(Self {
            protocol: env.get_field_id(&cls, "protocol", "Lcom/rex4j/RexConfig$Protocol;")?,
            host: env.get_field_id(&cls, "host", "Ljava/lang/String;")?,
            port: env.get_field_id(&cls, "port", "I")?,
            title: env.get_field_id(&cls, "title", "Ljava/lang/String;")?,
            idle_timeout: env.get_field_id(&cls, "idleTimeout", "J")?,
            pong_wait: env.get_field_id(&cls, "pongWait", "J")?,
            max_reconnect_attempts: env.get_field_id(&cls, "maxReconnectAttempts", "I")?,
            max_buffer_size: env.get_field_id(&cls, "maxBufferSize", "I")?,
            protocol_value: env.get_field_id(&protocol_cls, "value", "I")?,
        })
    }
}
