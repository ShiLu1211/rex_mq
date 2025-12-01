use pyo3::prelude::*;

#[pymodule]
fn rex4p(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(hello, m)?)?;
    Ok(())
}

#[pyfunction]
fn hello() -> PyResult<String> {
    Ok("hello".into())
}
