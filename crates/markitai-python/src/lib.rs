use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use std::panic::{AssertUnwindSafe, catch_unwind};

#[pyfunction]
fn convert_json(py: Python<'_>, request: String) -> PyResult<String> {
    py.detach(move || {
        catch_unwind(AssertUnwindSafe(|| markitai_core::convert_json(&request)))
            .map_err(|_| PyRuntimeError::new_err("Native conversion failed unexpectedly"))
    })
}

#[pyfunction]
fn config_json(overrides: &str) -> PyResult<String> {
    let overrides: serde_json::Value = serde_json::from_str(overrides)
        .map_err(|error| PyValueError::new_err(error.to_string()))?;
    if !overrides.is_object() {
        return Err(PyValueError::new_err("config must be an object"));
    }
    let mut config = markitai_core::config::defaults();
    markitai_core::config::merge(&mut config, overrides);
    markitai_core::config::validate(&config)
        .map_err(|error| PyValueError::new_err(error.to_string()))?;
    Ok(config.to_string())
}

#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(convert_json, module)?)?;
    module.add_function(wrap_pyfunction!(config_json, module)?)?;
    module.add("__version__", markitai_core::VERSION)?;
    Ok(())
}
