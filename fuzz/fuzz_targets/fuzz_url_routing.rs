#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        // Fuzz URL path parsing — must never panic
        let parts: Vec<&str> = s.split('/').collect();
        for part in &parts {
            // Simulate path parameter extraction
            let _ = part.parse::<u64>();
            let _ = urlencoding_decode(part);
        }
        // Simulate collection/document path extraction
        if parts.len() >= 3 {
            let _collection = parts.get(2);
            let _doc_id = parts.get(3);
        }
    }
});

fn urlencoding_decode(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            } else {
                result.push('%');
                result.push_str(&hex);
            }
        } else {
            result.push(c);
        }
    }
    result
}
