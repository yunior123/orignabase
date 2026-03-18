#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Fuzz JSON parsing — ensure no panics on arbitrary input
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = serde_json::from_str::<serde_json::Value>(s);
        // Also test deeply nested extraction
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
            let _ = v.get("collection");
            let _ = v.get("filter");
            let _ = v.get("sort");
            let _ = v.get("limit");
            let _ = v.get("data");
        }
    }
});
