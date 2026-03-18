#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        // Fuzz GraphQL filter/query string parsing
        let v: Result<serde_json::Value, _> = serde_json::from_str(s);
        if let Ok(v) = v {
            // Simulate filter extraction
            if let Some(filter) = v.get("filter") {
                let _ = filter.as_object();
                let _ = filter.as_array();
                let _ = filter.as_str();
            }
            if let Some(query) = v.get("query") {
                let _ = query.as_str();
            }
        }
    }
});
