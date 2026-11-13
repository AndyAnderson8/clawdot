//! Telling OpenAI's service from one that only mentions it (REQ-CFG-081).

use super::is_openai;

#[test]
fn openai_is_recognised_by_its_host_and_nothing_else() {
    for yes in [
        "https://api.openai.com/v1",
        "https://API.OpenAI.com",
        "https://api.openai.com:443/v1/audio",
        "http://api.openai.com/v1",
    ] {
        assert!(is_openai(yes), "{yes}");
    }
    for no in [
        "https://speech.example/v1/api.openai.com",
        "https://api.openai.com.evil.example/v1",
        "https://evil.example/v1?u=api.openai.com",
        "https://notapi.openai.com/v1",
        "https://api.openai.com@evil.example/v1",
        "https://evil.example/api.openai.com",
        "api.openai.com",
        "",
    ] {
        assert!(!is_openai(no), "{no}");
    }
}
