//! Dropping a wasmer store must free what was instantiated in it. wasmer
//! 7.1.0's wasmi backend leaked every store whose instance had exports or
//! host functions, linear memory included
//! (`patches/wasmer-7.1.0-wasmi-store-ownership.patch`).
//!
//! The only test in its binary, so no other test's allocations land in the
//! heap figures.
#![cfg(all(target_os = "linux", target_env = "gnu"))]

use wasmer::{imports, Function, FunctionEnv, FunctionEnvMut, Instance, Module, Store};

/// Exports a 2 MiB memory and imports a host function, as every zome does.
const WAT: &str = r#"
(module
  (import "env" "host" (func $host (param i32) (result i32)))
  (memory (export "memory") 32)
  (func (export "run") (param i32) (result i32)
    (call $host (local.get 0))))
"#;

/// Bytes the allocator has handed out and not had back.
fn live_heap() -> usize {
    let m = unsafe { libc::mallinfo2() };
    m.uordblks + m.hblkhd
}

fn instantiate_and_call(engine: &wasmer::Engine, module: &Module, rounds: i32) {
    for i in 0..rounds {
        let mut store = Store::new(engine.clone());
        let env = FunctionEnv::new(&mut store, ());
        let host = Function::new_typed_with_env(&mut store, &env, |_: FunctionEnvMut<()>, x: i32| x + 1);
        let instance = Instance::new(&mut store, module, &imports! { "env" => { "host" => host } }).unwrap();
        let run = instance.exports.get_typed_function::<i32, i32>(&store, "run").unwrap();
        assert_eq!(run.call(&mut store, i).unwrap(), i + 1);
    }
}

#[test]
fn a_dropped_store_frees_its_instance() {
    let engine = wasmer::Engine::default();
    let module = Module::new(&engine, WAT).unwrap();
    instantiate_and_call(&engine, &module, 20);
    let before = live_heap();
    instantiate_and_call(&engine, &module, 200);
    let grown = live_heap().saturating_sub(before);
    // Leaking each store would keep its memory: 400 MiB over 200 rounds.
    assert!(
        grown < 4 << 20,
        "live heap grew {} KiB over 200 instantiations",
        grown >> 10
    );
}
