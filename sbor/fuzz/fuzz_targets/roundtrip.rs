#![no_main]
use libfuzzer_sys::fuzz_target;
use sbor::{decode_any, encode_any};

fuzz_target!(|data: &[u8]| {
    let _ = fuzz(data);
});

fn fuzz(data: &[u8]) -> Result<(), ()> {
    let value = decode_any(data).map_err(|_| ())?;

    //let mut bytes = Vec::new();
    //let mut encoder = Encoder::with_type_id(&mut bytes);
    let bytes = encode_any(&value);

    let value2 = decode_any(&bytes).expect("encoded bytes to deserialize");
    assert_eq!(value, value2, "roundtrip failure");

    Ok(())
}
