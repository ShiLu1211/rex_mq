use pyo3::exceptions::PyException;
use pyo3::prelude::*;

mod client;
mod handler;
mod types;

pub use client::PyRexClient;
pub use handler::PyClientHandler;
pub use types::*;

/// Rex Python 客户端模块
#[pymodule]
fn rex4p(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // 注册类型
    m.add_class::<PyRexClient>()?;
    m.add_class::<PyClientConfig>()?;
    m.add_class::<PyProtocol>()?;
    m.add_class::<PyConnectionState>()?;
    m.add_class::<PyRexData>()?;
    m.add_class::<PyRexCommand>()?;
    m.add_class::<PyRetCode>()?;

    // 注册异常
    m.add("RexError", m.py().get_type::<PyRexError>())?;

    Ok(())
}

// 自定义异常类型
pyo3::create_exception!(rex_client, PyRexError, PyException);
