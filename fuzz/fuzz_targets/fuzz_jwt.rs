#![no_main]
use libfuzzer_sys::fuzz_target;
use ob_auth::jwt::{JwtKeys, verify_token};

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let keys = JwtKeys::from_secret("fuzz_test_secret_key_12345");
        // Must never panic on arbitrary token strings
        let _ = verify_token(s, &keys);
    }
});
