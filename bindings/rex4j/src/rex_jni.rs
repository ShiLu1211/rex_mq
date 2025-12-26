use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use bytes::{Bytes, BytesMut};
use jni::{
    JNIEnv,
    objects::{JByteArray, JClass, JObject, JString},
    sys::{jbyteArray, jint, jlong},
};
use rex_client::{RexClientConfig, RexClientTrait, open_client};
use rex_core::{Protocol, RexCommand, RexData};
use tokio::runtime::Runtime;
use tracing::{error, info};

use crate::rex_cache::RexGlobalCache;
use crate::rex_handler::JavaHandler;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn get_runtime() -> Result<&'static Runtime> {
    RUNTIME
        .get()
        .ok_or_else(|| anyhow::anyhow!("Tokio Runtime is not initialized"))
}

fn init_runtime() -> Result<()> {
    if RUNTIME.get().is_some() {
        return Ok(());
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Failed to build Tokio runtime")?;

    let _ = RUNTIME.set(runtime);
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_rex4j_jni_RexNative_init(
    mut env: JNIEnv,
    _class: JClass,
    config_obj: JObject,
    handler_obj: JObject,
) -> jlong {
    if let Err(e) = RexGlobalCache::init(&mut env) {
        error!("Failed to initialize cache: {:#}", e);
        let _ = throw_exception(&mut env, &format!("Cache init failed: {:#}", e));
        return 0;
    }

    if let Err(e) = init_runtime() {
        error!("Failed to initialize runtime: {:#}", e);
        let _ = throw_exception(&mut env, &format!("Runtime init failed: {:#}", e));
        return 0;
    }

    match init_internal(&mut env, &config_obj, &handler_obj) {
        Ok(handle) => handle,
        Err(e) => {
            error!("Init failed: {:#}", e);
            let _ = throw_exception(&mut env, &format!("Init failed: {:#}", e));
            0
        }
    }
}

fn init_internal(env: &mut JNIEnv, config_obj: &JObject, handler_obj: &JObject) -> Result<jlong> {
    let cache = RexGlobalCache::get().ok_or_else(|| anyhow::anyhow!("Cache not initialized"))?;

    let client_obj = env.alloc_object(&cache.client.cls)?;
    env.set_field_unchecked(
        &client_obj,
        cache.client.config,
        jni::objects::JValueGen::Object(config_obj),
    )?;
    env.set_field_unchecked(
        &client_obj,
        cache.client.handler,
        jni::objects::JValueGen::Object(handler_obj),
    )?;

    let protocol_obj = env
        .get_field_unchecked(
            config_obj,
            cache.config.protocol,
            jni::signature::ReturnType::Object,
        )?
        .l()?;
    let protocol_val = env
        .get_field_unchecked(
            &protocol_obj,
            cache.config.protocol_value,
            jni::signature::ReturnType::Primitive(jni::signature::Primitive::Int),
        )?
        .i()?;

    let host_obj = env
        .get_field_unchecked(
            config_obj,
            cache.config.host,
            jni::signature::ReturnType::Object,
        )?
        .l()?;
    let host_str: String = env.get_string(&JString::from(host_obj))?.into();

    let port = env
        .get_field_unchecked(
            config_obj,
            cache.config.port,
            jni::signature::ReturnType::Primitive(jni::signature::Primitive::Int),
        )?
        .i()?;

    let title_obj = env
        .get_field_unchecked(
            config_obj,
            cache.config.title,
            jni::signature::ReturnType::Object,
        )?
        .l()?;
    let title_str: String = env.get_string(&JString::from(title_obj))?.into();

    let idle_timeout = env
        .get_field_unchecked(
            config_obj,
            cache.config.idle_timeout,
            jni::signature::ReturnType::Primitive(jni::signature::Primitive::Long),
        )?
        .j()? as u64;

    let pong_wait = env
        .get_field_unchecked(
            config_obj,
            cache.config.pong_wait,
            jni::signature::ReturnType::Primitive(jni::signature::Primitive::Long),
        )?
        .j()? as u64;

    let max_reconnect = env
        .get_field_unchecked(
            config_obj,
            cache.config.max_reconnect_attempts,
            jni::signature::ReturnType::Primitive(jni::signature::Primitive::Int),
        )?
        .i()? as u32;

    let max_buffer_size = env
        .get_field_unchecked(
            config_obj,
            cache.config.max_buffer_size,
            jni::signature::ReturnType::Primitive(jni::signature::Primitive::Int),
        )?
        .i()? as usize;

    let protocol = match protocol_val {
        0 => Protocol::Tcp,
        1 => Protocol::Quic,
        2 => Protocol::WebSocket,
        _ => Protocol::Tcp,
    };

    let server_addr = format!("{}:{}", host_str, port)
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid server address: {}", e))?;

    let handler = JavaHandler::new(env, handler_obj, &client_obj)?;

    let mut config = RexClientConfig::new(protocol, server_addr, &title_str, handler.clone());
    config.idle_timeout = idle_timeout;
    config.pong_wait = pong_wait;
    config.max_reconnect_attempts = max_reconnect;
    config.max_buffer_size = max_buffer_size;

    info!("Opening client with config: {:?}", config.server_addr);

    let client = get_runtime()?.block_on(open_client(config))?;

    info!("Client opened successfully");

    let client_ptr = Box::into_raw(Box::new(client)) as jlong;

    env.set_field_unchecked(
        &client_obj,
        cache.client.client,
        jni::objects::JValueGen::Long(client_ptr),
    )?;

    Ok(client_ptr)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_rex4j_jni_RexNative_send(
    mut env: JNIEnv,
    _class: JClass,
    client_handle: jlong,
    data_obj: JObject,
) {
    if let Err(e) = send_internal(&mut env, client_handle, &data_obj) {
        error!("Send failed: {:#}", e);
        let _ = throw_exception(&mut env, &format!("Send failed: {:#}", e));
    }
}

fn send_internal(env: &mut JNIEnv, client_handle: jlong, data_obj: &JObject) -> Result<()> {
    if client_handle == 0 {
        return Err(anyhow::anyhow!("Invalid client handle"));
    }

    let client = unsafe { &*(client_handle as *const Arc<dyn RexClientTrait>) };
    let cache = RexGlobalCache::get().ok_or_else(|| anyhow::anyhow!("Cache not initialized"))?;

    let command_enum_obj = env
        .get_field_unchecked(
            data_obj,
            cache.data.command,
            jni::signature::ReturnType::Object,
        )?
        .l()?;
    let command = if command_enum_obj.is_null() {
        0
    } else {
        env.get_field_unchecked(
            &command_enum_obj,
            cache.command.value,
            jni::signature::ReturnType::Primitive(jni::signature::Primitive::Int),
        )?
        .i()?
    };
    let _ = env.delete_local_ref(command_enum_obj);

    let title_obj = env
        .get_field_unchecked(
            data_obj,
            cache.data.title,
            jni::signature::ReturnType::Object,
        )?
        .l()?;
    let title = if title_obj.is_null() {
        String::new()
    } else {
        env.get_string(<&JString>::from(&title_obj))?.into()
    };
    let _ = env.delete_local_ref(title_obj);

    let data_bytes_obj = env
        .get_field_unchecked(data_obj, cache.data.data, jni::signature::ReturnType::Array)?
        .l()?;
    let data_bytes = if data_bytes_obj.is_null() {
        Vec::new()
    } else {
        env.convert_byte_array(<&JByteArray>::from(&data_bytes_obj))?
    };
    let _ = env.delete_local_ref(data_bytes_obj);

    let data_bytesmut = BytesMut::from(Bytes::from(data_bytes));

    let rex_command = RexCommand::try_from(command as u32)
        .map_err(|_| anyhow::anyhow!("Invalid command: {}", command))?;

    let mut rex_data = RexData::builder(rex_command)
        .title(title)
        .data(data_bytesmut)
        .build();

    get_runtime()?.block_on(client.send_data(&mut rex_data))?;

    Ok(())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_rex4j_jni_RexNative_getConnectionState(
    _env: JNIEnv,
    _class: JClass,
    client_handle: jlong,
) -> jint {
    if client_handle == 0 {
        return 0;
    }

    let client = unsafe { &*(client_handle as *const Arc<dyn RexClientTrait>) };

    client.get_connection_state() as jint
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_rex4j_jni_RexNative_close(
    _env: JNIEnv,
    _class: JClass,
    client_handle: jlong,
) {
    if client_handle == 0 {
        return;
    }

    info!("Closing client");

    let client = unsafe { Box::from_raw(client_handle as *mut Arc<dyn RexClientTrait>) };

    match get_runtime() {
        Ok(rt) => {
            rt.block_on(client.close());
            info!("Client closed");
        }
        Err(e) => {
            error!("Runtime missing during close, skipped async close: {:#}", e);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_rex4j_jni_RexNative_registerTitle(
    mut env: JNIEnv,
    _class: JClass,
    client_handle: jlong,
    title: JString,
) {
    if let Err(e) = register_title_internal(&mut env, client_handle, &title) {
        error!("Register title failed: {:#}", e);
        let _ = throw_exception(&mut env, &format!("Register title failed: {:#}", e));
    }
}

fn register_title_internal(env: &mut JNIEnv, client_handle: jlong, title: &JString) -> Result<()> {
    if client_handle == 0 {
        return Err(anyhow::anyhow!("Invalid client handle"));
    }

    let client = unsafe { &*(client_handle as *const Arc<dyn RexClientTrait>) };
    let title_str: String = env.get_string(title)?.into();

    let mut data = RexData::builder(RexCommand::RegTitle)
        .data_from_string(title_str)
        .build();

    get_runtime()?.block_on(client.send_data(&mut data))?;

    Ok(())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_rex4j_jni_RexNative_deleteTitle(
    mut env: JNIEnv,
    _class: JClass,
    client_handle: jlong,
    title: JString,
) {
    if let Err(e) = delete_title_internal(&mut env, client_handle, &title) {
        error!("Delete title failed: {:#}", e);
        let _ = throw_exception(&mut env, &format!("Delete title failed: {:#}", e));
    }
}

fn delete_title_internal(env: &mut JNIEnv, client_handle: jlong, title: &JString) -> Result<()> {
    if client_handle == 0 {
        return Err(anyhow::anyhow!("Invalid client handle"));
    }

    let client = unsafe { &*(client_handle as *const Arc<dyn RexClientTrait>) };
    let title_str: String = env.get_string(title)?.into();

    let mut data = RexData::builder(RexCommand::DelTitle)
        .data_from_string(title_str)
        .build();

    get_runtime()?.block_on(client.send_data(&mut data))?;

    Ok(())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_rex4j_jni_RexNative_getClientId(
    mut env: JNIEnv,
    _class: JClass,
    client_handle: jlong,
) -> jbyteArray {
    match get_client_id_internal(&mut env, client_handle) {
        Ok(arr) => arr,
        Err(e) => {
            error!("Get client ID failed: {:#}", e);
            let _ = throw_exception(&mut env, &format!("Get client ID failed: {:#}", e));
            JObject::null().into_raw()
        }
    }
}

fn get_client_id_internal(env: &mut JNIEnv, _client_handle: jlong) -> Result<jbyteArray> {
    let id_bytes: [u8; 16] = [0; 16];
    let jarray = env.new_byte_array(16)?;
    let signed_bytes: Vec<i8> = id_bytes.iter().map(|&b| b as i8).collect();
    env.set_byte_array_region(&jarray, 0, &signed_bytes)?;
    Ok(jarray.into_raw())
}

fn throw_exception(env: &mut JNIEnv, msg: &str) -> Result<()> {
    env.throw_new("com/rex4j/exception/RexException", msg)?;
    Ok(())
}
