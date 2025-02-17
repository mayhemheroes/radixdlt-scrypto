(module
  (import "env" "radix_engine" (func $radix_engine (param i32) (result i32)))

  ;; scrypto_encode(&RadixEngineInput::EmitLog(Level::Debug, "Hello".to_string()));
  (data (i32.const 1024) "\11\07\00\00\00EmitLog\02\00\00\00\11\05\00\00\00Debug\00\00\00\00\0c\05\00\00\00Hello")

  ;; Simple function that always returns `()`
  (func $Test_f (param $0 i32) (result i32)
    (local $buffer i32)
    (local.set 
      $buffer
      (call $scrypto_alloc
        (i32.const 40)
      )
    )
    (drop
      (call $memcpy
        (i32.add
          (local.get $buffer)
          (i32.const 4)
        )
        (i32.const 1024)
        (i32.const 40)
      )
    )
    (drop
      (call $radix_engine
        (local.get $buffer)
      )
    )

    ;; Return unit
    (local.set 
      $buffer
      (call $scrypto_alloc
        (i32.const 1)
      )
    )
    (i32.add
      (local.get $buffer)
      (i32.const 4)
    )
    (i32.const 0)
    (i32.store8)
    (local.get $buffer)
  )

  (memory $0 1)
  (export "memory" (memory $0))
  (export "scrypto_alloc" (func $scrypto_alloc))
  (export "scrypto_free" (func $scrypto_free))
  (export "Test_f" (func $Test_f))

  ${memcpy}
  ${buffer}
)