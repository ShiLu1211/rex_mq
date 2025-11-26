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
    pub get_value: JMethodID,
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
            get_value: env.get_method_id(&cls_local, "getValue", "()I")?,
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
        let cls_local = env.find_class("com/rex4j/RexHandler")?;
        let _cls = env.new_global_ref(&cls_local)?;

        Ok(Self {
            on_login: env.get_method_id(
                &cls_local,
                "onLogin",
                "(Lcom/rex4j/RexClient;Lcom/rex4j/RexData;)V",
            )?,
            on_message: env.get_method_id(
                &cls_local,
                "onMessage",
                "(Lcom/rex4j/RexClient;Lcom/rex4j/RexData;)V",
            )?,
        })
    }
}

pub struct RexConfigCache {
    // pub cls: GlobalRef,
    // pub protocol_cls: GlobalRef,
    pub get_protocol: JMethodID,
    pub get_host: JMethodID,
    pub get_port: JMethodID,
    pub get_title: JMethodID,
    pub get_idle_timeout: JMethodID,
    pub get_pong_wait: JMethodID,
    pub get_max_reconnect_attempts: JMethodID,
    pub get_read_buffer_size: JMethodID,
    pub get_max_buffer_size: JMethodID,

    pub get_protocol_value: JMethodID,
}

impl RexConfigCache {
    pub fn init(env: &mut JNIEnv) -> Result<Self> {
        let cls_local = env.find_class("com/rex4j/RexConfig")?;
        // let cls = env.new_global_ref(&cls_local)?;

        let protocol_local = env.find_class("com/rex4j/RexConfig$Protocol")?;
        // let protocol_cls = env.new_global_ref(&protocol_local)?;

        Ok(Self {
            // cls,
            // protocol_cls,
            get_protocol: env.get_method_id(
                &cls_local,
                "getProtocol",
                "()Lcom/rex4j/RexConfig$Protocol;",
            )?,

            get_host: env.get_method_id(&cls_local, "getHost", "()Ljava/lang/String;")?,
            get_port: env.get_method_id(&cls_local, "getPort", "()I")?,
            get_title: env.get_method_id(&cls_local, "getTitle", "()Ljava/lang/String;")?,

            get_idle_timeout: env.get_method_id(&cls_local, "getIdleTimeout", "()J")?,
            get_pong_wait: env.get_method_id(&cls_local, "getPongWait", "()J")?,
            get_max_reconnect_attempts: env.get_method_id(
                &cls_local,
                "getMaxReconnectAttempts",
                "()I",
            )?,

            get_read_buffer_size: env.get_method_id(&cls_local, "getReadBufferSize", "()I")?,
            get_max_buffer_size: env.get_method_id(&cls_local, "getMaxBufferSize", "()I")?,

            get_protocol_value: env.get_method_id(&protocol_local, "getValue", "()I")?,
        })
    }
}
