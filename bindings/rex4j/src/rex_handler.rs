use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use jni::{
    JNIEnv,
    objects::{GlobalRef, JObject, JValue},
};
use rex_client::RexClientHandlerTrait;
use rex_core::{RexClientInner, RexCommand, RexData, RexDataRef};
use rkyv::AlignedVec;
use tracing::warn;

use crate::rex_cache::RexGlobalCache;

/// Java Handler 的 Rust 包装器
pub struct JavaHandler {
    jvm: Arc<jni::JavaVM>,
    handler_obj: GlobalRef,
    client_obj: GlobalRef,

    tx: kanal::Sender<Bytes>,
}

impl JavaHandler {
    pub fn new(env: &mut JNIEnv, handler: &JObject, client: &JObject) -> Result<Arc<Self>> {
        let jvm = env.get_java_vm()?;
        let handler_obj = env.new_global_ref(handler)?;
        let client_obj = env.new_global_ref(client)?;

        let (tx, rx) = kanal::bounded(10000);

        let jvm_thread = Arc::new(jvm);
        let jvm = jvm_thread.clone();

        let handler = Arc::new(Self {
            jvm,
            handler_obj,
            client_obj,
            tx,
        });
        let handler_clone = handler.clone();

        std::thread::spawn(move || {
            match jvm_thread.attach_current_thread_as_daemon() {
                Ok(mut env) => {
                    let cache = match RexGlobalCache::get() {
                        Some(c) => c,
                        None => {
                            warn!("RexGlobalCache not initialized in handler thread");
                            return;
                        }
                    };

                    let mut aligned = AlignedVec::with_capacity(64 * 1024);

                    while let Ok(serialized_bytes) = rx.recv() {
                        if let Err(e) = (|| -> Result<()> {
                            // ✅ 转换为对齐缓冲区（一次拷贝）
                            aligned.clear();
                            aligned.extend_from_slice(&serialized_bytes);

                            // ✅ 零拷贝访问 - 无反序列化开销
                            let data_ref = RexData::as_archived(&aligned);

                            // 创建 Java RexData 对象
                            let data_obj = env.alloc_object(&cache.data.cls)?;
                            handler.init_java_data(&mut env, data_ref, &data_obj)?;

                            let args = [
                                JValue::Object(&handler.client_obj).as_jni(),
                                JValue::Object(&data_obj).as_jni(),
                            ];

                            // 调用 Java: void onMessage(RexClient client, RexData data)
                            unsafe {
                                env.call_method_unchecked(
                                    &handler.handler_obj,
                                    cache.handler.on_message,
                                    jni::signature::ReturnType::Primitive(
                                        jni::signature::Primitive::Void,
                                    ),
                                    &args[..],
                                )?;
                            }

                            let _ = env.delete_local_ref(data_obj);

                            Ok(())
                        })() {
                            warn!("Java handler invocation failed: {}", e);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to attach handler thread to JVM: {}", e);
                }
            }
        });

        Ok(handler_clone)
    }

    /// 创建 Java RexData 对象
    fn init_java_data(
        &self,
        env: &mut JNIEnv,
        data_ref: RexDataRef<'_>,
        data_obj: &JObject,
    ) -> Result<()> {
        let cache =
            RexGlobalCache::get().ok_or_else(|| anyhow::anyhow!("Cache not initialized"))?;

        // 设置 command 字段
        let command_int = data_ref.command() as i32;
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
            data_obj,
            cache.data.command,
            JValue::Object(&command_enum_obj),
        )?;
        let _ = env.delete_local_ref(command_enum_obj);

        // 设置 title 字段
        let title_str = env.new_string(data_ref.title().unwrap_or_default())?;
        env.set_field_unchecked(data_obj, cache.data.title, JValue::Object(&title_str))?;
        let _ = env.delete_local_ref(title_str);

        // 设置 data 字段
        let data_bytes = data_ref.data();
        let jbytes = env.byte_array_from_slice(data_bytes)?;
        env.set_field_unchecked(data_obj, cache.data.data, JValue::Object(&jbytes))?;
        let _ = env.delete_local_ref(jbytes);

        Ok(())
    }

    fn call_on_login(&self, serialized_bytes: &Bytes) -> Result<()> {
        // 转换为对齐缓冲区
        let mut aligned = AlignedVec::with_capacity(serialized_bytes.len());
        aligned.extend_from_slice(serialized_bytes);

        // 零拷贝访问
        let data_ref = RexData::as_archived(&aligned);

        let mut env = self.jvm.attach_current_thread()?;
        let cache =
            RexGlobalCache::get().ok_or_else(|| anyhow::anyhow!("Cache not initialized"))?;

        // 创建 Java RexData 对象
        let data_obj = env.alloc_object(&cache.data.cls)?;
        self.init_java_data(&mut env, data_ref, &data_obj)?;

        let args = [
            JValue::Object(&self.client_obj).as_jni(),
            JValue::Object(&data_obj).as_jni(),
        ];

        // 调用 Java: void onLogin(RexClient client, RexData data)
        unsafe {
            env.call_method_unchecked(
                &self.handler_obj,
                cache.handler.on_login,
                jni::signature::ReturnType::Primitive(jni::signature::Primitive::Void),
                &args[..],
            )
        }?;

        let _ = env.delete_local_ref(data_obj);

        Ok(())
    }
}

unsafe impl Send for JavaHandler {}
unsafe impl Sync for JavaHandler {}

#[async_trait::async_trait]
impl RexClientHandlerTrait for JavaHandler {
    async fn login_ok(&self, _client: Arc<RexClientInner>, data: RexData) -> Result<()> {
        let serialized = data.serialize();
        if let Err(e) = self.call_on_login(&serialized) {
            warn!("Failed to call onLogin: {}", e);
            // 不返回 Err 给 Rust Core，防止断开连接，只记录日志
        }
        Ok(())
    }

    async fn handle(&self, _client: Arc<RexClientInner>, data: RexData) -> Result<()> {
        if matches!(data.command(), RexCommand::Check | RexCommand::CheckReturn) {
            return Ok(());
        }
        let serialized = data.serialize();
        if let Err(e) = self.tx.send(serialized) {
            warn!("Failed to enqueue message for Java handler: {}", e);
        }
        Ok(())
    }
}
