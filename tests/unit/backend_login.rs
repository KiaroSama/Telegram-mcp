use super::*;

#[tokio::test]
async fn rejected_login_input_retries_only_up_to_three_attempts() {
    let mut attempts = 0;
    let result = retry_auth(
        async || {
            attempts += 1;
            Ok((attempts == 2).then_some(7))
        },
        "exhausted",
    )
    .await
    .unwrap();
    assert_eq!((result, attempts), (7, 2));

    let mut attempts = 0;
    let failure = retry_auth::<()>(
        async || {
            attempts += 1;
            Ok(None)
        },
        "exhausted",
    )
    .await
    .unwrap_err();
    assert_eq!(failure.to_string(), "exhausted");
    assert_eq!(attempts, 3);
}

#[tokio::test]
async fn failed_login_transport_is_not_retried_as_rejected_input() {
    let mut attempts = 0;
    let failure = retry_auth::<()>(
        async || {
            attempts += 1;
            bail!("transport failure");
        },
        "exhausted",
    )
    .await
    .unwrap_err();
    assert_eq!(failure.to_string(), "transport failure");
    assert_eq!(attempts, 1);
}
