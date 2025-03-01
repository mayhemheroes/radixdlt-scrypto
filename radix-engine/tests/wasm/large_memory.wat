(module

  ;; Simple function that always returns `()`
  (func $Test_f (param $0 i32) (result i32)
    (local $buffer i32)
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

  (memory $0 100)
  (export "memory" (memory $0))
  (export "scrypto_alloc" (func $scrypto_alloc))
  (export "scrypto_free" (func $scrypto_free))
  (export "Test_f" (func $Test_f))

  ${memcpy}
  ${buffer}
)