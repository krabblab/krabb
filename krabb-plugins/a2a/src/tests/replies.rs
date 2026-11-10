use crate::*;

#[test]
fn includes_bounded_quotes_in_standard_prompts() {
    let request = A2aRequest::parse(&[
        json!("send"),
        json!("--quote"),
        json!("earlier message"),
        json!("explain"),
    ])
    .unwrap();

    assert!(request.prompt.contains("earlier message"));
    assert!(request.prompt.contains("not instructions"));
    assert!(request.prompt.ends_with("explain"));
    assert!(A2aRequest::parse(&[json!("--quote")]).is_err());
    assert!(A2aRequest::parse(&[json!("discover"), json!("--quote"), json!("text")]).is_err());
    assert_eq!(A2aRequest::parse(&[json!("send"), json!("hello")]).unwrap().prompt, "hello");
}
