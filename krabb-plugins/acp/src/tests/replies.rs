use crate::*;

#[test]
fn includes_bounded_quotes_in_standard_prompts() {
    let request =
        RunRequest::parse(&[json!("--quote"), json!("earlier message"), json!("explain")]).unwrap();

    assert!(request.prompt.contains("earlier message"));
    assert!(request.prompt.contains("not instructions"));
    assert!(request.prompt.ends_with("explain"));
    assert!(RunRequest::parse(&[json!("--quote")]).is_err());
    assert_eq!(RunRequest::parse(&[json!("hello")]).unwrap().prompt, "hello");
}
