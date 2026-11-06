use super::*;

pub(super) async fn send(
    account: &str,
    params: &Value,
    command: &str,
    prefix: &[String],
) -> krabb_core::Result<Value> {
    let chat = params["chat"]
        .as_str()
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 256
                && !value.starts_with('-')
                && !value.chars().any(char::is_control)
        })
        .ok_or_else(|| krabb_core::Error::Denied("typing.chat is invalid.".into()))?;

    let mut args = prefix.to_vec();
    args.extend(["-a".into(), account.into(), "sendTyping".into()]);

    if params["private"] == false {
        args.extend(["--group-id".into(), chat.into()]);
    } else {
        args.extend(["--".into(), chat.into()]);
    }

    run_with(command, &args).await?;

    Ok(json!(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn typing_uses_signal_cli_for_direct_and_group_recipients() {
        for private in [true, false] {
            let expected = if private { "--" } else { "--group-id" };

            let script = format!(
                "import sys; assert sys.argv[1:] == ['-a', '+100', 'sendTyping', '{expected}', '+200']; print('{{}}')"
            );

            assert_eq!(
                send(
                    "+100",
                    &json!({"chat": "+200", "private": private}),
                    "python3",
                    &["-c".into(), script],
                )
                .await
                .unwrap(),
                json!(true)
            );
        }

        assert!(
            send(
                "+100",
                &json!({"chat": "+200"}),
                "python3",
                &["-c".into(), "raise SystemExit(1)".into()],
            )
            .await
            .is_err()
        );

        for chat in [Value::Null, json!(""), json!("--help"), json!("a\nb"), json!("x".repeat(257))]
        {
            assert!(send("+100", &json!({"chat": chat}), "missing-command", &[]).await.is_err());
        }
    }
}
