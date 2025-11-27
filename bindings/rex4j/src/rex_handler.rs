use std::sync::Arc;

use anyhow::Result;
use jni::{
    JNIEnv,
    objects::{GlobalRef, JObject, JValue},
};
use rex_client::RexClientHandlerTrait;
use rex_client::RexClientInner;
use rex_core::{RexCommand, RexData};
use tracing::warn;

use crate::rex_cache::RexGlobalCache;

/// Java Handler 的 Rust 包装器
pub struct JavaHandler {
    jvm: Arc<jni::JavaVM>,
    handler_obj: GlobalRef,
    config_obj: GlobalRef,
    client_ptr: std::sync::RwLock<i64>, // 使用 RwLock 允许后续更新
}

impl JavaHandler {
    pub fn new(env: &mut JNIEnv, handler: &JObject, config: &JObject) -> Result<Self> {
        let jvm = env.get_java_vm()?;
        let handler_obj = env.new_global_ref(handler)?;
        let config_obj = env.new_global_ref(config)?;
        Ok(Self {
            jvm: Arc::new(jvm),
            handler_obj,
            config_obj,
            client_ptr: std::sync::RwLock::new(0),
        })
    }

    /// 设置客户端指针（在客户端创建后调用）
    pub fn set_client_ptr(&self, client_ptr: i64) -> Result<()> {
        let mut guard = self
            .client_ptr
            .write()
            .map_err(|_| anyhow::anyhow!("Handler lock is poisoned"))?;
        *guard = client_ptr;
        Ok(())
    }

    /// 创建 Java RexData 对象
    fn create_java_data<'a>(&self, env: &mut JNIEnv<'a>, data: &RexData) -> Result<JObject<'a>> {
        let cache =
            RexGlobalCache::get().ok_or_else(|| anyhow::anyhow!("Cache not initialized"))?;

        // 创建 RexData 对象
        let data_obj = env.new_object(&cache.data.cls, "()V", &[])?;

        // 设置 command 字段
        let command_int = data.header().command() as i32;
        let command_enum_obj = unsafe {
            env.call_static_method_unchecked(
                &cache.command.cls,
                cache.command.from_value,
                jni::signature::ReturnType::Object,
                &[JValue::Int(command_int).as_jni()],
            )?
        }
        .l()?;
        env.set_field_unchecked(
            &data_obj,
            cache.data.command,
            JValue::Object(&command_enum_obj),
        )?;

        // 设置 title 字段
        let title_str = env.new_string(data.title().unwrap_or_default())?;
        env.set_field_unchecked(&data_obj, cache.data.title, JValue::Object(&title_str))?;

        // 设置 data 字段
        let data_bytes = data.data();
        let jbytes = env.byte_array_from_slice(data_bytes)?;
        env.set_field_unchecked(&data_obj, cache.data.data, JValue::Object(&jbytes))?;

        Ok(data_obj)
    }

    fn call_on_login(&self, data: &RexData) -> Result<()> {
        let mut env = self.jvm.attach_current_thread()?;

        let cache =
            RexGlobalCache::get().ok_or_else(|| anyhow::anyhow!("Cache not initialized"))?;

        // 获取客户端指针，处理锁中毒
        let client_ptr = {
            let guard = self
                .client_ptr
                .read()
                .map_err(|_| anyhow::anyhow!("Handler lock is poisoned"))?;
            *guard
        };

        if client_ptr == 0 {
            warn!("Client pointer not set yet, skipping callback");
            return Ok(());
        }

        // 创建 Java RexClient 对象
        let client_obj = self.create_java_client_with_ptr(&mut env, client_ptr)?;

        // 创建 Java RexData 对象
        let data_obj = self.create_java_data(&mut env, data)?;

        // 调用 Java: void onLogin(RexClient client, RexData data)
        unsafe {
            env.call_method_unchecked(
                &self.handler_obj,
                cache.handler.on_login,
                jni::signature::ReturnType::Primitive(jni::signature::Primitive::Void),
                &[
                    JValue::Object(&client_obj).as_jni(),
                    JValue::Object(&data_obj).as_jni(),
                ],
            )
        }?;

        Ok(())
    }

    fn call_on_message(&self, data: &RexData) -> Result<()> {
        let mut env = self.jvm.attach_current_thread()?;

        let cache =
            RexGlobalCache::get().ok_or_else(|| anyhow::anyhow!("Cache not initialized"))?;

        // 获取客户端指针，处理锁中毒
        let client_ptr = {
            let guard = self
                .client_ptr
                .read()
                .map_err(|_| anyhow::anyhow!("Handler lock is poisoned"))?;
            *guard
        };

        if client_ptr == 0 {
            return Ok(());
        }

        // 创建 Java RexClient 对象
        let client_obj = self.create_java_client_with_ptr(&mut env, client_ptr)?;

        // 创建 Java RexData 对象
        let data_obj = self.create_java_data(&mut env, data)?;

        // 调用 Java: void onMessage(RexClient client, RexData data)
        unsafe {
            env.call_method_unchecked(
                &self.handler_obj,
                cache.handler.on_message,
                jni::signature::ReturnType::Primitive(jni::signature::Primitive::Void),
                &[
                    JValue::Object(&client_obj).as_jni(),
                    JValue::Object(&data_obj).as_jni(),
                ],
            )
        }?;

        Ok(())
    }

    /// 通过指针创建 Java RexClient 引用对象
    fn create_java_client_with_ptr<'a>(
        &self,
        env: &mut JNIEnv<'a>,
        client_ptr: i64,
    ) -> Result<JObject<'a>> {
        let cache =
            RexGlobalCache::get().ok_or_else(|| anyhow::anyhow!("Cache not initialized"))?;

        // 使用 alloc_object 创建对象，不调用构造函数
        let client_obj = env.alloc_object(&cache.client.cls)?;

        // 设置 client 指针字段
        env.set_field_unchecked(&client_obj, cache.client.client, JValue::Long(client_ptr))?;

        // 设置 Config
        env.set_field_unchecked(
            &client_obj,
            cache.client.config,
            JValue::Object(self.config_obj.as_obj()),
        )?;

        // 设置 Handler
        env.set_field_unchecked(
            &client_obj,
            cache.client.handler,
            JValue::Object(self.handler_obj.as_obj()),
        )?;

        Ok(client_obj)
    }
}

unsafe impl Send for JavaHandler {}
unsafe impl Sync for JavaHandler {}

#[async_trait::async_trait]
impl RexClientHandlerTrait for JavaHandler {
    async fn login_ok(&self, _client: Arc<RexClientInner>, data: RexData) -> Result<()> {
        if let Err(e) = self.call_on_login(&data) {
            warn!("Failed to call onLogin: {}", e);
            // 不返回 Err 给 Rust Core，防止断开连接，只记录日志
        }
        Ok(())
    }

    async fn handle(&self, _client: Arc<RexClientInner>, data: RexData) -> Result<()> {
        if matches!(data.header().command(), RexCommand::Check | RexCommand::CheckReturn) {
            return Ok(())
        }
        if let Err(e) = self.call_on_message(&data) {
            warn!("Failed to call onMessage: {}", e);
        }
        Ok(())
    }
}
