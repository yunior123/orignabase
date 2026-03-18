#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        // Fuzz the security rules parser — must never panic
        let _ = ob_security::parser::parse_rules(s);
    }
});
