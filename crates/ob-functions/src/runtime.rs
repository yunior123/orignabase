use ob_core::{Error, Result};
use wasmi::*;

/// Configuration for WASM function execution limits.
#[derive(Debug, Clone)]
pub struct FunctionLimits {
    /// Maximum fuel (instruction count) for execution.
    pub max_fuel: u64,
    /// Maximum wall-clock time in seconds.
    pub max_time_secs: u64,
}

impl Default for FunctionLimits {
    fn default() -> Self {
        Self {
            max_fuel: 1_000_000_000,
            max_time_secs: 30,
        }
    }
}

/// The WASM runtime engine. Shared across all function invocations.
pub struct WasmRuntime {
    engine: Engine,
    limits: FunctionLimits,
}

impl WasmRuntime {
    /// Create a new WASM runtime with the given limits.
    pub fn new(limits: FunctionLimits) -> Result<Self> {
        let mut config = Config::default();
        config.consume_fuel(true);

        let engine =
            Engine::new(&config);

        Ok(Self { engine, limits })
    }

    /// Compile a WASM module from bytes.
    pub fn compile(&self, wasm_bytes: &[u8]) -> Result<Module> {
        Module::new(&self.engine, wasm_bytes)
            .map_err(|e| Error::Internal(format!("WASM compile error: {e}")))
    }

    /// Execute a WASM function with fuel limits.
    /// The function should export `alloc(i32) -> i32` and `{function_name}(i32, i32) -> i64`.
    /// Input is passed as UTF-8 bytes, result is read back as UTF-8.
    pub async fn execute(
        &self,
        module: &Module,
        function_name: &str,
        input: &str,
    ) -> Result<String> {
        let engine = self.engine.clone();
        let module = module.clone();
        let function_name = function_name.to_string();
        let input = input.to_string();
        let limits = self.limits.clone();

        // Run in blocking task since wasmi is sync
        tokio::task::spawn_blocking(move || {
            execute_sync(&engine, &module, &function_name, &input, &limits)
        })
        .await
        .map_err(|e| Error::Internal(format!("WASM task join error: {e}")))?
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn limits(&self) -> &FunctionLimits {
        &self.limits
    }
}

/// Synchronous WASM execution with fuel metering.
fn execute_sync(
    engine: &Engine,
    module: &Module,
    function_name: &str,
    input: &str,
    limits: &FunctionLimits,
) -> Result<String> {
    let mut store = Store::new(engine, ());

    // Set fuel limit
    store.set_fuel(limits.max_fuel).map_err(|e| Error::Internal(format!("Set fuel: {e}")))?;

    let linker = Linker::new(engine);
    let instance = linker
        .instantiate(&mut store, module)
        .map_err(|e| Error::Internal(format!("WASM instantiate: {e}")))?
        .start(&mut store)
        .map_err(|e| Error::Internal(format!("WASM start: {e}")))?;

    // Get the memory export
    let memory = instance
        .get_memory(&store, "memory")
        .ok_or_else(|| Error::Internal("WASM module has no memory export".into()))?;

    // Write input to WASM memory via alloc
    let alloc = instance
        .get_typed_func::<i32, i32>(&store, "alloc")
        .map_err(|e| Error::Internal(format!("No alloc export: {e}")))?;

    let input_bytes = input.as_bytes();
    let input_len = input_bytes.len() as i32;
    let input_ptr = alloc
        .call(&mut store, input_len)
        .map_err(|e| Error::Internal(format!("alloc failed: {e}")))?;

    memory
        .data_mut(&mut store)
        .get_mut(input_ptr as usize..input_ptr as usize + input_bytes.len())
        .ok_or_else(|| Error::Internal("WASM memory write out of bounds".into()))?
        .copy_from_slice(input_bytes);

    // Call the function: (ptr, len) -> i64 (packed ptr+len)
    let func = instance
        .get_typed_func::<(i32, i32), i64>(&store, function_name)
        .map_err(|e| Error::Internal(format!("Function '{function_name}' not found: {e}")))?;

    let result_packed = func
        .call(&mut store, (input_ptr, input_len))
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("fuel") {
                Error::Internal("WASM function exceeded CPU fuel limit".into())
            } else {
                Error::Internal(format!("WASM call error: {msg}"))
            }
        })?;

    // Unpack ptr+len from i64 (high 32 = ptr, low 32 = len)
    let result_ptr = (result_packed >> 32) as usize;
    let result_len = (result_packed & 0xFFFF_FFFF) as usize;

    // Read result from WASM memory
    let data = memory.data(&store);
    let result_bytes = data
        .get(result_ptr..result_ptr + result_len)
        .ok_or_else(|| Error::Internal("WASM memory read out of bounds".into()))?;

    String::from_utf8(result_bytes.to_vec())
        .map_err(|e| Error::Internal(format!("Invalid UTF-8 from WASM: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_limits() {
        let limits = FunctionLimits::default();
        assert_eq!(limits.max_time_secs, 30);
        assert_eq!(limits.max_fuel, 1_000_000_000);
    }

    #[test]
    fn test_engine_creation() {
        let runtime = WasmRuntime::new(FunctionLimits::default());
        assert!(runtime.is_ok());
    }

    #[test]
    fn test_compile_invalid_wasm() {
        let runtime = WasmRuntime::new(FunctionLimits::default()).unwrap();
        let result = runtime.compile(b"not valid wasm");
        assert!(result.is_err());
    }

    #[test]
    fn test_compile_minimal_wasm() {
        let runtime = WasmRuntime::new(FunctionLimits::default()).unwrap();
        // Minimal valid WASM module (magic + version + empty)
        let wasm = wat::parse_str("(module)").unwrap();
        let result = runtime.compile(&wasm);
        assert!(result.is_ok());
    }

    /// WAT module that echoes input back: alloc returns fixed offset 1024,
    /// process returns (ptr << 32) | len pointing at the same input data.
    const ECHO_WAT: &str = r#"(module
        (memory (export "memory") 1)
        (func (export "alloc") (param $size i32) (result i32)
            i32.const 1024
        )
        (func (export "process") (param $ptr i32) (param $len i32) (result i64)
            local.get $ptr
            i64.extend_i32_u
            i64.const 32
            i64.shl
            local.get $len
            i64.extend_i32_u
            i64.or
        )
    )"#;

    #[tokio::test]
    async fn test_execute_echo() {
        let runtime = WasmRuntime::new(FunctionLimits::default()).unwrap();
        let wasm = wat::parse_str(ECHO_WAT).unwrap();
        let module = runtime.compile(&wasm).unwrap();
        let result = runtime.execute(&module, "process", "hello").await.unwrap();
        assert_eq!(result, "hello");
    }

    #[tokio::test]
    async fn test_execute_empty_input() {
        let runtime = WasmRuntime::new(FunctionLimits::default()).unwrap();
        let wasm = wat::parse_str(ECHO_WAT).unwrap();
        let module = runtime.compile(&wasm).unwrap();
        let result = runtime.execute(&module, "process", "").await.unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_custom_limits() {
        let limits = FunctionLimits {
            max_fuel: 500,
            max_time_secs: 10,
        };
        assert_eq!(limits.max_fuel, 500);
        assert_eq!(limits.max_time_secs, 10);

        let runtime = WasmRuntime::new(limits).unwrap();
        assert_eq!(runtime.limits().max_fuel, 500);
        assert_eq!(runtime.limits().max_time_secs, 10);
    }

    #[test]
    fn test_engine_accessor() {
        let runtime = WasmRuntime::new(FunctionLimits::default()).unwrap();
        // Just verify engine() returns a reference without panicking
        let _engine = runtime.engine();
    }

    #[test]
    fn test_limits_accessor() {
        let limits = FunctionLimits {
            max_fuel: 42,
            max_time_secs: 7,
        };
        let runtime = WasmRuntime::new(limits).unwrap();
        let got = runtime.limits();
        assert_eq!(got.max_fuel, 42);
        assert_eq!(got.max_time_secs, 7);
    }

    #[tokio::test]
    async fn test_execute_missing_function() {
        let runtime = WasmRuntime::new(FunctionLimits::default()).unwrap();
        let wasm = wat::parse_str(ECHO_WAT).unwrap();
        let module = runtime.compile(&wasm).unwrap();
        // "nonexistent" is not exported by the module
        let result = runtime.execute(&module, "nonexistent", "hello").await;
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("nonexistent"), "Error should mention the missing function name: {err_msg}");
    }
}
